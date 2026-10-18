//! Tests for the `misc` hook.

use super::*;

/// Doctor: corrupt gain rows and interleaved debug.log lines are surfaced.
#[test]
fn doctor_flags_telemetry_anomalies() {
    let home = tmp("doctele");
    let dir = home.join(".tokenade");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("gain.jsonl"),
        "{\"op\":\"x\",\"before_tokens\":10,\"after_tokens\":99}\nnot-json\n",
    )
    .unwrap();
    let mut dbg = String::new();
    for _ in 0..50 {
        dbg.push_str("{\"ok\":1}\n");
    }
    for _ in 0..5 {
        dbg.push_str("{corrupt{{\n");
    }
    fs::write(dir.join("debug.log"), dbg).unwrap();
    let r = crate::healthcheck::check_local_log_integrity(&home);
    assert!(!r.detail.is_empty(), "expected anomalies");
    assert!(
        r.detail.iter().any(|d| d.contains("corrupt")),
        "{:?}",
        r.detail
    );
}

// Detection is path-driven and pure — pin every method against a realistic
// executable path so npm never auto-deletes a binary npm owns, and a
// standalone curl-install is correctly self-removed. (`detect_install_method`,
// `remove_tokenade_data`, `InstallMethod` are in scope via `crate::install::*`.)
#[test]
fn detect_install_method_by_path() {
    use crate::install::uninstall::InstallMethod;
    use std::path::Path as P;
    assert_eq!(
        detect_install_method(P::new(
            "/usr/lib/node_modules/@tokenade/cli/vendor/tokenade"
        )),
        InstallMethod::Npm
    );
    assert_eq!(
        detect_install_method(P::new(
            "C:\\Users\\a\\AppData\\Roaming\\npm\\node_modules\\@tokenade\\cli\\vendor\\tokenade.exe"
        )),
        InstallMethod::Npm
    );
    assert_eq!(
        detect_install_method(P::new("/home/u/.cargo/bin/tokenade")),
        InstallMethod::Cargo
    );
    assert_eq!(
        detect_install_method(P::new("/opt/homebrew/Cellar/tokenade/0.8.11/bin/tokenade")),
        InstallMethod::Brew
    );
    assert_eq!(
        detect_install_method(P::new("/usr/local/bin/tokenade")),
        InstallMethod::Standalone
    );
    assert_eq!(
        detect_install_method(P::new("/home/u/.local/bin/tokenade")),
        InstallMethod::Standalone
    );
}

// A binary running from a cargo build tree (dev checkout / e2e harness) must
// NEVER be self-deleted by `uninstall` — that nukes a developer's build and,
// in the e2e suite, the shared binary sibling tests spawn. Guard is path-based
// so it holds regardless of dry_run.
#[test]
fn binary_removal_never_deletes_build_artifact() {
    use crate::install::uninstall::{binary_removal_action, InstallMethod};
    use std::path::Path as P;
    for p in [
        "/home/u/proj/target/release/tokenade",
        "/home/u/proj/target/debug/tokenade",
        "C:\\proj\\target\\release\\tokenade.exe",
    ] {
        let (method, line) = binary_removal_action(P::new(p), false);
        assert_eq!(method, InstallMethod::Standalone, "{p}");
        assert!(
            line.contains("build tree"),
            "{p}: expected guard, got: {line}"
        );
        assert!(
            !line.contains("deleted the standalone"),
            "{p}: must not self-delete"
        );
    }
    // A real install location that doesn't exist on disk: dry_run reports the
    // intended delete without touching anything.
    let (_, dry) = binary_removal_action(P::new("/home/u/.local/bin/tokenade"), true);
    assert!(dry.contains("would delete"), "got: {dry}");
}

// Full data wipe removes BOTH tokenade-owned trees and NOTHING else in HOME or
// the cache dir.
#[test]
fn remove_tokenade_data_wipes_only_tokenade_dirs() {
    let home = tmp("wipehome");
    let cache = tmp("wipecache");
    // tokenade-owned data
    fs::create_dir_all(home.join(".tokenade")).unwrap();
    fs::write(home.join(".tokenade/gain.jsonl"), "x").unwrap();
    fs::create_dir_all(cache.join("tokenade/stash")).unwrap();
    fs::write(cache.join("tokenade/stash/deadbeef.txt"), "x").unwrap();
    // bystanders that MUST survive
    fs::write(home.join(".bashrc"), "mine").unwrap();
    fs::create_dir_all(home.join(".config/other")).unwrap();
    fs::write(home.join(".config/other/keep.toml"), "keep").unwrap();
    fs::write(cache.join("other-app.log"), "keep").unwrap();

    let lines = remove_tokenade_data(Some(&home), Some(&cache), false);
    assert_eq!(lines.len(), 2, "two dirs deleted: {lines:?}");
    assert!(lines.iter().all(|l| l.starts_with("deleted")), "{lines:?}");
    assert!(!home.join(".tokenade").exists(), "~/.tokenade must be gone");
    assert!(
        !cache.join("tokenade").exists(),
        "cache/tokenade must be gone"
    );
    // bystanders intact
    assert_eq!(fs::read_to_string(home.join(".bashrc")).unwrap(), "mine");
    assert!(home.join(".config/other/keep.toml").is_file());
    assert!(cache.join("other-app.log").is_file());
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cache);
}

// `--keep-data` path == not calling the wipe: the ledger survives. And
// `--dry-run` lists the exact targets while touching nothing.
#[test]
fn remove_tokenade_data_dry_run_lists_but_keeps() {
    let home = tmp("dryhome");
    let cache = tmp("drycache");
    fs::create_dir_all(home.join(".tokenade")).unwrap();
    fs::write(home.join(".tokenade/gain.jsonl"), "x").unwrap();
    fs::create_dir_all(cache.join("tokenade")).unwrap();

    let lines = remove_tokenade_data(Some(&home), Some(&cache), true);
    assert_eq!(lines.len(), 2);
    assert!(
        lines.iter().all(|l| l.starts_with("would delete")),
        "{lines:?}"
    );
    assert!(
        home.join(".tokenade/gain.jsonl").is_file(),
        "dry-run kept data"
    );
    assert!(cache.join("tokenade").is_dir(), "dry-run kept cache");
    // Absent dirs → no lines, no panic (idempotent second pass).
    let empty = remove_tokenade_data(Some(&home.join("nope")), Some(&cache.join("nope")), false);
    assert!(empty.is_empty());
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cache);
}

/// The uninstall lines must say HOW MUCH, not just where.
///
/// `~/.tokenade` holds the local-agent model weights — 20.3 GB on a dogfood
/// install — and the line read `would delete /home/x/.tokenade` with no hint of
/// it. A dry-run is exactly the moment someone is deciding, and a bare path does
/// not tell them what they are deciding about.
#[test]
fn remove_tokenade_data_says_how_much_it_will_reclaim() {
    let home = tmp("sizehome");
    let cache = tmp("sizecache");
    fs::create_dir_all(home.join(".tokenade/models")).unwrap();
    // 3 MiB, so the human formatter is past its byte range and prints MB.
    fs::write(
        home.join(".tokenade/models/weights.bin"),
        vec![0u8; 3 << 20],
    )
    .unwrap();
    fs::create_dir_all(cache.join("tokenade")).unwrap();

    let dry = remove_tokenade_data(Some(&home), Some(&cache), true);
    let line = dry
        .iter()
        .find(|l| l.contains(".tokenade"))
        .unwrap_or_else(|| panic!("no .tokenade line in {dry:?}"));
    assert!(line.contains("3.0 MB"), "size missing from {line:?}");

    let real = remove_tokenade_data(Some(&home), Some(&cache), false);
    let line = real
        .iter()
        .find(|l| l.contains(".tokenade"))
        .unwrap_or_else(|| panic!("no .tokenade line in {real:?}"));
    assert!(
        line.contains("3.0 MB reclaimed"),
        "the real run must report what came back, got {line:?}"
    );
    assert!(!home.join(".tokenade").exists());
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cache);
}

// Symlink safety: a link INSIDE ~/.tokenade that points OUTSIDE must be
// unlinked, never followed — the off-target file survives.
#[cfg(unix)]
#[test]
fn remove_tokenade_data_never_follows_symlink_off_target() {
    let home = tmp("lnkhome");
    let outside = tmp("lnkoutside");
    fs::write(outside.join("precious.txt"), "do not delete").unwrap();
    fs::create_dir_all(home.join(".tokenade")).unwrap();
    std::os::unix::fs::symlink(&outside, home.join(".tokenade/escape")).unwrap();

    let lines = remove_tokenade_data(Some(&home), None, false);
    assert!(lines.iter().any(|l| l.starts_with("deleted")), "{lines:?}");
    assert!(!home.join(".tokenade").exists(), "~/.tokenade removed");
    assert!(
        outside.join("precious.txt").is_file(),
        "symlink target must NOT be deleted"
    );
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&outside);
}

// FAIL-OPEN: an un-removable data dir yields a "couldn't" line and leaves the
// tree in place, but the function returns normally (never panics / aborts the
// uninstall). Uses a write-protected container; skipped when perms are
// unenforced (running as root / on a filesystem that ignores mode bits).
#[cfg(unix)]
#[test]
fn remove_tokenade_data_fail_open_on_unremovable() {
    use std::os::unix::fs::PermissionsExt;
    let home = tmp("failhome");
    let td = home.join(".tokenade");
    fs::create_dir_all(&td).unwrap();
    fs::write(td.join("locked.jsonl"), "x").unwrap();
    // Drop write on the container so the child file can't be unlinked.
    fs::set_permissions(&td, fs::Permissions::from_mode(0o500)).unwrap();
    // Probe: if we can still create a file here, perms aren't enforced → skip.
    let enforced = fs::write(td.join(".probe"), "p").is_err();
    if !enforced {
        let _ = fs::set_permissions(&td, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&home);
        return;
    }
    let lines = remove_tokenade_data(Some(&home), None, false);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("couldn't"),
        "expected fail-open line: {lines:?}"
    );
    assert!(td.exists(), "un-removable dir stays put");
    // Restore so the tempdir can be cleaned.
    let _ = fs::set_permissions(&td, fs::Permissions::from_mode(0o700));
    let _ = fs::remove_dir_all(&home);
}

/// Uninstall restores a proxied home byte-for-byte: the posix + fish rc lines
/// a previous release installed are stripped clean and the fragment files are
/// deleted. The installer is gone (the proxy is out of the product), so this
/// migration is the ONLY thing that gets those machines back — the rc content
/// below is what real ≤0.9.4 installs wrote, verbatim.
#[test]
fn shell_proxy_uninstall_restores_rc_files() {
    let home = tmp("proxyhome");
    let bashrc = home.join(".bashrc");
    let fishdir = home.join(".config/fish");
    fs::create_dir_all(&fishdir).unwrap();
    let fishrc = fishdir.join("config.fish");
    fs::create_dir_all(home.join(".tokenade")).unwrap();
    fs::write(home.join(".tokenade/shellproxy.sh"), "# legacy bootstrap\n").unwrap();

    // Exactly what the (removed) installer appended, user content around it.
    let posix_line = "\n# tokenade shell proxy\nexport BASH_ENV=\"$HOME/.tokenade/shellproxy.sh\"\nif command -v tokenade >/dev/null 2>&1; then eval \"$(tokenade shell-init --shell sh)\" || true; fi\n";
    let fish_line = "\n# tokenade shell proxy\nset -gx BASH_ENV $HOME/.tokenade/shellproxy.sh\ntokenade shell-init --shell fish | source\n";
    fs::write(&bashrc, format!("# my prompt\nexport FOO=1\n{posix_line}")).unwrap();
    fs::write(&fishrc, format!("set -g fish_greeting\n{fish_line}")).unwrap();

    uninstall_shell_proxy(Some(&home), false);

    assert_eq!(
        fs::read_to_string(&bashrc).unwrap(),
        "# my prompt\nexport FOO=1\n"
    );
    assert_eq!(
        fs::read_to_string(&fishrc).unwrap(),
        "set -g fish_greeting\n"
    );
    assert!(!home.join(".tokenade/shellproxy.sh").exists());
    let _ = fs::remove_dir_all(&home);
}

/// Field report: a PRE-bootstrap install pasted the whole `__tokenade_proxy`
/// function INLINE into `.zshrc` (no `~/.tokenade/shellproxy.sh` sentinel).
/// `uninstall_shell_proxy` must remove the crashing (FUNCNEST-unsafe) block
/// whole even with no fragment file to key on — otherwise the old function
/// stays live in every shell after an upgrade and keeps aborting on nested
/// calls.
#[test]
fn legacy_inline_proxy_stripped_without_sentinel() {
    let home = tmp("proxyinline");
    let zshrc = home.join(".zshrc");
    let inline = "# user top\nexport EDITOR=vim\n\
__tokenade_proxy() {\n\
  if [ -t 0 ] || [ \"${TOKENADE_WRAPPING:-0}\" = \"1\" ]; then\n\
    command \"$@\"; return $?\n\
  fi\n\
  TOKENADE_WRAPPING=1 tokenade wrap \"$@\"\n\
}\n\
for __tc in git cargo cd cat; do alias \"$__tc\"=\"__tokenade_proxy $__tc\"; done\n\
unset __tc\n\
alias ll='ls -la'\n";
    fs::write(&zshrc, inline).unwrap();

    // No fragment-file sentinel exists — the strip must not depend on one.
    assert!(!home.join(".tokenade/shellproxy.sh").exists());

    uninstall_shell_proxy(Some(&home), false);
    let after = fs::read_to_string(&zshrc).unwrap();
    assert!(
        !after.contains("__tokenade_proxy"),
        "inline block survived strip:\n{after}"
    );
    // User content on both sides of the block is preserved.
    assert!(
        after.contains("export EDITOR=vim"),
        "top user line lost:\n{after}"
    );
    assert!(
        after.contains("alias ll='ls -la'"),
        "bottom user line lost:\n{after}"
    );
    // Idempotent: a second strip is a no-op, not a further mutation.
    uninstall_shell_proxy(Some(&home), false);
    assert_eq!(
        fs::read_to_string(&zshrc).unwrap(),
        after,
        "second strip drifted"
    );
    let _ = fs::remove_dir_all(&home);
}

/// Upgrade migration: a machine carrying BOTH legacy blocks (the ≤0.8.6 alias
/// eval AND a ≤0.9.4 shim PATH prepend) gets both ripped out, keeping the
/// user's own lines on either side. This is the state the shim's removal has
/// to clean up in the field.
#[test]
fn path_shim_uninstall_migrates_legacy_blocks() {
    let home = tmp("shimmigrate");
    fs::create_dir_all(&home).unwrap();
    let bashrc = home.join(".bashrc");
    let legacy = "# my prompt\nexport FOO=1\n\n# tokenade shell proxy\nexport BASH_ENV=\"$HOME/.tokenade/shellproxy.sh\"\nif command -v tokenade >/dev/null 2>&1; then eval \"$(tokenade shell-init --shell sh)\" || true; fi\n\n# tokenade path shim\ncase \":$PATH:\" in *\":$HOME/.tokenade/shim:\"*) ;; *) PATH=\"$HOME/.tokenade/shim:$PATH\"; export PATH ;; esac\nexport BAR=2\n";
    fs::write(&bashrc, legacy).unwrap();
    fs::create_dir_all(home.join(".tokenade")).unwrap();
    fs::write(home.join(".tokenade/shellproxy.sh"), "# legacy\n").unwrap();
    fs::create_dir_all(shim_dir(&home)).unwrap();
    fs::write(
        shim_dir(&home).join("git"),
        "#!/bin/sh\n# tokenade shim v1\n",
    )
    .unwrap();

    uninstall_shell_proxy(Some(&home), false);
    uninstall_path_shim(Some(&home), false);

    let b = fs::read_to_string(&bashrc).unwrap();
    assert!(
        b.contains("export FOO=1") && b.contains("export BAR=2"),
        "user lines lost:\n{b}"
    );
    assert!(
        !b.contains("shellproxy"),
        "legacy alias line survived:\n{b}"
    );
    assert!(!b.contains("shell-init"), "legacy eval survived:\n{b}");
    assert!(!b.contains("path shim"), "shim block survived:\n{b}");
    assert!(
        !b.contains(".tokenade/shim"),
        "shim PATH prepend survived:\n{b}"
    );
    assert!(
        !home.join(".tokenade/shellproxy.sh").exists(),
        "legacy fragment not deleted"
    );
    assert!(!shim_dir(&home).exists(), "shim dir not removed");
    let _ = fs::remove_dir_all(&home);
}

/// Uninstall restores the rc file byte-for-byte (no leftover blank line) and
/// is idempotent — a second pass on an already-clean home is a no-op.
#[test]
fn path_shim_uninstall_restores_rc_exactly() {
    let home = tmp("shimroundtrip");
    fs::create_dir_all(&home).unwrap();
    let bashrc = home.join(".bashrc");
    let shim_block = "\n# tokenade path shim\ncase \":$PATH:\" in *\":$HOME/.tokenade/shim:\"*) ;; *) PATH=\"$HOME/.tokenade/shim:$PATH\"; export PATH ;; esac\n";
    fs::write(&bashrc, format!("# prompt\nexport FOO=1\n{shim_block}")).unwrap();
    fs::create_dir_all(shim_dir(&home)).unwrap();

    uninstall_path_shim(Some(&home), false);
    assert_eq!(
        fs::read_to_string(&bashrc).unwrap(),
        "# prompt\nexport FOO=1\n"
    );
    assert!(!shim_dir(&home).exists(), "shim dir not removed");

    // Idempotent: nothing left to strip, nothing further mutated.
    uninstall_path_shim(Some(&home), false);
    assert_eq!(
        fs::read_to_string(&bashrc).unwrap(),
        "# prompt\nexport FOO=1\n"
    );
    let _ = fs::remove_dir_all(&home);
}

/// The shim rc-line matcher and the legacy alias matcher are DISJOINT, so
/// migration never mistakes one block for the other.
#[test]
fn path_shim_line_matcher_disjoint_from_legacy() {
    // Shim lines match the shim matcher, not the alias matcher.
    for l in [
        "# tokenade path shim",
        "case \":$PATH:\" in *\":$HOME/.tokenade/shim:\"*) ;; *) PATH=\"$HOME/.tokenade/shim:$PATH\"; export PATH ;; esac",
    ] {
        assert!(shim_path_line_is_tokenade(l), "shim matcher missed: {l}");
        assert!(!shell_proxy_line_is_tokenade(l), "alias matcher grabbed a shim line: {l}");
    }
    // Legacy alias fragment lines match the alias matcher, not the shim one.
    for l in [
        "export BASH_ENV=\"$HOME/.tokenade/shellproxy.sh\"",
        "alias git=\"__tokenade_proxy git\"",
    ] {
        assert!(
            !shim_path_line_is_tokenade(l),
            "shim matcher grabbed an alias line: {l}"
        );
    }
}

#[test]
fn output_mode_parse_accepts_all_forms_and_aliases() {
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // absent → Full (back-compat).
    assert_eq!(
        parse_output_mode(&a(&["Foo"]), "query"),
        Ok(OutputMode::Full)
    );
    // space-separated + `=`-joined, long + short + --format alias.
    assert_eq!(
        parse_output_mode(&a(&["-o", "paths"]), "query"),
        Ok(OutputMode::Paths)
    );
    assert_eq!(
        parse_output_mode(&a(&["--output-mode=count"]), "map"),
        Ok(OutputMode::Count)
    );
    assert_eq!(
        parse_output_mode(&a(&["--format", "outline"]), "impact"),
        Ok(OutputMode::Outline)
    );
    assert_eq!(
        parse_output_mode(&a(&["-o=paths_only"]), "query"),
        Ok(OutputMode::Paths)
    );
    assert_eq!(
        parse_output_mode(&a(&["-o", "signatures"]), "query"),
        Ok(OutputMode::Outline)
    );
    assert_eq!(
        parse_output_mode(&a(&["--output-mode", "full"]), "query"),
        Ok(OutputMode::Full)
    );
    // unknown value → hard error (exit 2), not silent Full.
    assert_eq!(parse_output_mode(&a(&["-o", "bogus"]), "query"), Err(2));
    // flag with no value → error.
    assert_eq!(parse_output_mode(&a(&["--output-mode"]), "query"), Err(2));
}

#[test]
fn output_mode_query_modes_reduce_and_are_correct() {
    let results = vec![
        (
            "Foo".to_string(),
            vec![
                om_sym("fn", "foo", "src/b.rs", 10),
                om_sym("struct", "Foo", "src/a.rs", 3),
            ],
        ),
        ("Bar".to_string(), vec![om_sym("fn", "bar", "src/a.rs", 20)]),
    ];

    // A stand-in for the FULL render (per-symbol context lines). The
    // reduced modes must be strictly smaller than this.
    let full_ref = "fn foo — src/b.rs:10\n  <body line 1>\n  <body line 2>\n\
        struct Foo — src/a.rs:3\n  <body>\nfn bar — src/a.rs:20\n  <body>\n";

    // paths: deduped + sorted file list, nothing else.
    let paths = output_mode_body_query(OutputMode::Paths, &results, true);
    assert_eq!(paths, "src/a.rs\nsrc/b.rs\n");
    assert!(paths.len() < full_ref.len(), "paths must reduce");
    assert!(!paths.contains("fn"), "paths must not carry signatures");

    // count: one count per symbol (multi → labelled).
    let count = output_mode_body_query(OutputMode::Count, &results, true);
    assert_eq!(count, "Foo: 2\nBar: 1\n");
    assert!(count.len() < full_ref.len(), "count must reduce");
    // single-query count is a bare number.
    let single = vec![("Foo".to_string(), vec![om_sym("fn", "foo", "a.rs", 1)])];
    assert_eq!(
        output_mode_body_query(OutputMode::Count, &single, false),
        "1\n"
    );

    // outline: location lines, no bodies.
    let outline = output_mode_body_query(OutputMode::Outline, &results, true);
    assert!(outline.contains("fn foo — src/b.rs:10"));
    assert!(outline.contains("==> Foo <=="));
    assert!(!outline.contains("<body"), "outline must drop bodies");
    assert!(
        outline.len() < full_ref.len(),
        "outline must reduce vs a context-bearing full"
    );

    // Full is caller-handled: the pure helper emits nothing for it.
    assert_eq!(output_mode_body_query(OutputMode::Full, &results, true), "");
}

#[test]
fn output_mode_semantic_modes_reduce_and_are_correct() {
    let hits = vec![
        om_hit("src/a.rs", 1.23),
        om_hit("src/b.rs", 0.90),
        om_hit("src/a.rs", 0.40),
    ];
    // FULL render carries score + file + matched-terms per hit.
    let full_ref: String = hits
        .iter()
        .map(|h| format!("{:5.2}  {}  [{}]\n", h.score, h.file, h.matched.join(", ")))
        .collect();

    let paths = output_mode_body_semantic(OutputMode::Paths, &hits);
    assert_eq!(paths, "src/a.rs\nsrc/b.rs\n");
    assert!(paths.len() < full_ref.len());

    let count = output_mode_body_semantic(OutputMode::Count, &hits);
    assert_eq!(count, "3\n");
    assert!(count.len() < full_ref.len());

    let outline = output_mode_body_semantic(OutputMode::Outline, &hits);
    assert!(outline.contains("src/a.rs"));
    assert!(
        !outline.contains("[alpha"),
        "outline drops matched-terms explainability"
    );
    assert!(outline.len() < full_ref.len(), "outline must reduce");

    assert_eq!(output_mode_body_semantic(OutputMode::Full, &hits), "");
}

#[test]
fn output_mode_skeleton_count_helper() {
    let sk = "pub fn a();\npub fn b();\n\npub struct C;\n";
    // 3 non-empty signature lines; blank line ignored.
    assert_eq!(skeleton_signature_count(sk), 3);
    assert_eq!(skeleton_signature_count(""), 0);
}

// ── `wrap` fast-path routing (direct-exec vs `sh -c`) ──────────────────────
// Adversarial gate coverage: a MIS-route to Direct would run a shell-dependent
// command WITHOUT a shell (broken output / wrong exit / wrong split), so the
// traps below MUST all resolve to Shell. `wrap_route_inner` is the pure decision
// core (env + PATH injected) so the whole table is deterministic. Rule: Direct
// ONLY on proven-clean `word word word`; at the slightest doubt → Shell (slower,
// byte-identical downstream, never wrong). Bonus: skipping `sh -c` on clean
// commands shrinks the FUNCNEST/rc re-sourcing surface — keep that intact.
#[test]
fn wrap_route_direct_only_for_clean_argv() {
    use crate::wrap::{wrap_route_inner, Route};
    let on_path = |_: &str| true;
    for cmd in [
        "git log --oneline",
        "cargo build --release",
        "grep -n foo file.txt",
        "npm ci",
        "kubectl get pods -n prod",
        "ls -la",
        "git status",
        "git commit -m x",
    ] {
        assert_eq!(
            wrap_route_inner(cmd, None, false, on_path),
            Route::Direct,
            "{cmd:?} is clean argv → Direct"
        );
    }
}

#[test]
fn wrap_route_shell_for_every_metacharacter_trap() {
    use crate::wrap::{wrap_route_inner, Route};
    let on_path = |_: &str| true;
    for cmd in [
        // control operators / trailing
        "ls;",
        "a ; b",
        "a | b | c",
        "a && b || c",
        "sleep 1 &",
        "a & b",
        // hidden separators
        "a\nb",
        "a\tb",
        // redirections
        "a > f",
        "a >> f",
        "a < f",
        "a 2>&1",
        "cat <<EOF",
        // glob / expansion
        "ls *.txt",
        "rm foo?bar",
        "cat ~/f",
        "echo {a,b}",
        "echo $HOME",
        "echo ${X}",
        // command substitution / subshell
        "echo $(x)",
        "echo `x`",
        "(cd a && b)",
        // quotes (whitespace-split would be wrong)
        "git commit -m \"a b\"",
        "grep -r 'a b' .",
        "awk '{print $1}'",
        // misc
        "a # c",
        "echo !!",
        "back\\slash",
        "test [ -f x ]",
    ] {
        assert_eq!(
            wrap_route_inner(cmd, None, false, on_path),
            Route::Shell,
            "{cmd:?} carries a shell metachar → Shell"
        );
    }
}

#[test]
fn wrap_route_shell_for_env_assignments() {
    use crate::wrap::{wrap_route_inner, Route};
    let on_path = |_: &str| true;
    for cmd in [
        "FOO=bar cmd",
        "FOO=bar",
        "A=1 B=2 cmd",
        "PATH=/x:$PATH cmd",
        "_p=1 z",
    ] {
        assert_eq!(
            wrap_route_inner(cmd, None, false, on_path),
            Route::Shell,
            "{cmd:?} leads with an env assignment → Shell"
        );
    }
}

#[test]
fn wrap_route_shell_for_builtins() {
    use crate::wrap::{wrap_route_inner, Route};
    let on_path = |_: &str| true;
    for cmd in [
        "cd /tmp",
        ".",
        "source f",
        "export X=1",
        "eval x",
        "exec y",
        "time cmd",
        "set -e",
        "unset X",
        "exit 0",
        "read x",
        "local v",
    ] {
        assert_eq!(
            wrap_route_inner(cmd, None, false, on_path),
            Route::Shell,
            "{cmd:?} is a shell builtin → Shell"
        );
    }
    // `[ -f x ]` (the `test` builtin form) is also caught by the `[` metachar.
    assert_eq!(
        wrap_route_inner("[ -f x ]", None, false, on_path),
        Route::Shell
    );
}

#[test]
fn wrap_route_shell_when_argv0_absent_or_empty() {
    use crate::wrap::{wrap_route_inner, Route};
    let absent = |_: &str| false;
    assert_eq!(
        wrap_route_inner("nonexistent-xyz foo", None, false, absent),
        Route::Shell
    );
    assert_eq!(
        wrap_route_inner("", None, false, |_: &str| true),
        Route::Shell
    );
    assert_eq!(
        wrap_route_inner("   ", None, false, |_: &str| true),
        Route::Shell
    );
}

#[test]
fn wrap_route_shell_for_override_and_killswitch() {
    use crate::wrap::{wrap_route_inner, Route, ShellKind};
    let on_path = |_: &str| true;
    for k in [ShellKind::Posix, ShellKind::PowerShell, ShellKind::Cmd] {
        assert_eq!(
            wrap_route_inner("git status", Some(k), false, on_path),
            Route::Shell,
            "explicit --shell {k:?} → Shell (override)"
        );
    }
    assert_eq!(
        wrap_route_inner("git status", None, true, on_path),
        Route::Shell,
        "TOKENADE_WRAP_NO_FASTPATH kill-switch → Shell"
    );
}

#[test]
#[cfg(unix)]
fn wrap_route_resolves_real_path() {
    use crate::wrap::{wrap_route, Route};
    assert_eq!(wrap_route("ls -la", None), Route::Direct);
    assert_eq!(
        wrap_route("this-binary-does-not-exist-xyz", None),
        Route::Shell
    );
    assert_eq!(wrap_route("ls | wc -l", None), Route::Shell);
}

// `command_on_path` must resolve a bare name to its PATHEXT-backed binary
// (`git` → `git.exe`), and must NOT resolve a name with no backing file.
#[test]
#[cfg(windows)]
fn wrap_command_on_path_pathext_windows() {
    use crate::wrap::command_on_path;
    // cmd.exe lives in System32, always on PATH → resolves via `.EXE`.
    assert!(command_on_path("cmd"), "cmd → cmd.exe via PATHEXT");
    // where.exe is a real System32 program (not a cmd builtin).
    assert!(command_on_path("where"), "where → where.exe via PATHEXT");
    // No backing .exe/.cmd/.bat for a made-up name → unresolved.
    assert!(!command_on_path("tokenade-no-such-cmd-xyzzy"));
}

// End-to-end routing on the real Windows PATH: clean argv whose argv[0] is a
// real .exe → Direct; a pure cmd.exe builtin (no backing binary) → Shell, so
// it runs under `cmd.exe /C` instead of a broken `Command::new("dir")`.
#[test]
#[cfg(windows)]
fn wrap_route_windows_real_path_and_cmd_builtins() {
    use crate::wrap::{wrap_route, Route};
    // Real executables → Direct fast-path.
    assert_eq!(wrap_route("cmd /c ver", None), Route::Direct);
    assert_eq!(wrap_route("where git", None), Route::Direct);
    // Pure cmd.exe builtins have no .exe anywhere → argv0 unresolved → Shell.
    for b in ["dir", "type foo.txt", "copy a b", "echo hi", "cls", "del x"] {
        assert_eq!(
            wrap_route(b, None),
            Route::Shell,
            "{b:?}: cmd builtin has no binary → Shell (cmd.exe /C), not a broken direct exec"
        );
    }
    // Metacharacter (cmd pipe) → Shell regardless of platform.
    assert_eq!(wrap_route("dir | find \"x\"", None), Route::Shell);
    // Unresolvable argv0 → Shell (matching error message from the shell).
    assert_eq!(
        wrap_route("tokenade-no-such-cmd-xyzzy a", None),
        Route::Shell
    );
}

// The Claude Code PowerShell/cmd hooks pass `--shell`; it must ALWAYS force
// Shell so PowerShell/cmd syntax never runs under a bare `Command::new`.
#[test]
#[cfg(windows)]
fn wrap_route_windows_shell_override_forces_shell() {
    use crate::wrap::{wrap_route, Route, ShellKind};
    assert_eq!(
        wrap_route(
            "Get-ChildItem | Measure-Object",
            Some(ShellKind::PowerShell)
        ),
        Route::Shell
    );
    // Even a clean, resolvable argv is forced to Shell when --shell is set.
    assert_eq!(
        wrap_route("where git", Some(ShellKind::PowerShell)),
        Route::Shell
    );
    assert_eq!(wrap_route("dir", Some(ShellKind::Cmd)), Route::Shell);
}

// ---- SECURITY: `tokenade pack` bodies must be redacted ----
// read_packable is the single ingress for every packed file body (and it
// runs before budget tokenization), so redacting there scrubs any embedded
// `.env`/config secret from the bundle.
#[test]
fn read_packable_redacts_secrets() {
    let dir = std::env::temp_dir().join(format!("tkn_pack_redact_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("config.env");
    std::fs::write(
        &f,
        "AWS_SECRET=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\nAPI_KEY=sk-ant-SECRET123\n",
    )
    .unwrap();
    let body = crate::feature_cli::read_packable(&f, 1_000_000).expect("readable");
    assert!(
        !body.contains("wJalrXUtnFEMI"),
        "aws secret leaked into pack body: {body:?}"
    );
    assert!(
        !body.contains("sk-ant-SECRET123"),
        "api key leaked into pack body: {body:?}"
    );
    assert!(
        body.contains("<redacted>"),
        "expected redaction marker: {body:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_file_decodes_legacy_charsets_instead_of_destroying_them() {
    // Measured on the CrUX Japan top 1 000: 15 of 165 homepages are Shift-JIS,
    // EUC-JP or Windows-31J, and `tokenade read` returned pure mojibake for
    // every one — 33 643 U+FFFD across 12 pages. `from_utf8_lossy` was called
    // "best effort" for legacy charsets, but replacing each undecodable byte
    // with U+FFFD destroys the content rather than tolerating it.
    let dir = std::env::temp_dir().join(format!("tokenade-sjis-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sjis.html");

    // "ソニー製品情報" in Shift-JIS.
    let sjis: &[u8] = &[
        0x3c, 0x68, 0x74, 0x6d, 0x6c, 0x3e, 0x3c, 0x6d, 0x65, 0x74, 0x61, 0x20, 0x63, 0x68, 0x61,
        0x72, 0x73, 0x65, 0x74, 0x3d, 0x22, 0x53, 0x68, 0x69, 0x66, 0x74, 0x5f, 0x4a, 0x49, 0x53,
        0x22, 0x3e, 0x3c, 0x62, 0x6f, 0x64, 0x79, 0x3e, 0x3c, 0x70,
        0x3e, // <html><meta charset="Shift_JIS"><body><p>
        0x83, 0x5c, 0x83, 0x6a, 0x81, 0x5b, 0x90, 0xbb, 0x95, 0x69, 0x8f, 0xee, 0x95, 0xf1, 0x3c,
        0x2f, 0x70, 0x3e, 0x3c, 0x2f, 0x62, 0x6f, 0x64, 0x79, 0x3e, 0x3c, 0x2f, 0x68, 0x74, 0x6d,
        0x6c, 0x3e,
    ];
    std::fs::write(&path, sjis).unwrap();

    let got = crate::bounded_input::read_file_lossy(&path).expect("readable");
    assert!(
        got.contains("ソニー製品情報"),
        "legacy charset destroyed: {got}"
    );
    assert!(!got.contains('\u{FFFD}'), "replacement chars leaked: {got}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn meta_trailer_is_split_off_without_touching_the_body() {
    use tokenade_core::web_fetch::{split_meta_trailer, META_SENTINEL, META_SEP};
    let body = b"<html><body>real page bytes \xff\xfe binary-ish</body></html>";

    let mut raw = body.to_vec();
    raw.extend_from_slice(
        format!("{META_SENTINEL}404{META_SEP}https://example.com/gone").as_bytes(),
    );
    let f = split_meta_trailer(raw);
    assert_eq!(f.bytes, body, "body must survive byte-for-byte");
    assert_eq!(f.status, 404);
    assert_eq!(f.effective_url.as_deref(), Some("https://example.com/gone"));

    // No trailer at all — behave exactly as the old bytes-only fetch did.
    let f = split_meta_trailer(body.to_vec());
    assert_eq!(f.bytes, body);
    assert_eq!(f.status, 200);
    assert!(f.effective_url.is_none());

    // Malformed status, empty URL: fail open rather than mangle anything.
    let mut raw = body.to_vec();
    raw.extend_from_slice(format!("{META_SENTINEL}not-a-code{META_SEP}").as_bytes());
    let f = split_meta_trailer(raw);
    assert_eq!(f.bytes, body);
    assert_eq!(f.status, 200);
    assert!(f.effective_url.is_none());
}

/// Round 18 — the fetcher sent a full Chrome user-agent and then refused every
/// cookie, a combination no browser produces. Cookie-gated sites 302'd forever.
/// The jar must be unique per fetch (so concurrent fetches cannot read each
/// other's cookies) and must not survive the call.
#[test]
fn cookie_jars_are_unique_per_fetch_and_do_not_persist() {
    use tokenade_core::web_fetch::cookie_jar_path;

    let a = cookie_jar_path().expect("jar path");
    let b = cookie_jar_path().expect("jar path");
    assert_ne!(a, b, "two fetches must not share a jar: {a:?}");
    assert!(
        a.parent().is_some_and(|p| p.exists()),
        "jar dir created: {a:?}"
    );
    // Nothing is written until curl runs, so a fresh path must not exist —
    // a jar that pre-existed would be state carried between calls.
    assert!(!a.exists() && !b.exists(), "jar must not pre-exist");
}

/// Round 18 — `tokenade web` could not reach the site readers at all: the
/// fetch, the render and the HOOK's delivery were one function. Both channels
/// must now agree on WHICH urls have a reader, and on what that reader's
/// output is worth.
#[test]
fn both_channels_agree_on_which_urls_have_a_reader() {
    // The CLI decides with the same predicate the hook does. If these ever
    // diverge, one channel silently serves the generic pipeline for a domain
    // the other special-cases — which is exactly the round-18 defect.
    for url in [
        "https://crates.io/crates/serde",
        "https://github.com/rust-lang/rust/issues/1",
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
    ] {
        assert!(
            tokenade_core::site_readers::plan(url).is_some(),
            "reader expected for {url}"
        );
    }
    for url in [
        "https://example.com/",
        "https://man7.org/linux/man-pages/man2/open.2.html",
    ] {
        assert!(
            tokenade_core::site_readers::plan(url).is_none(),
            "no reader expected for {url}"
        );
    }
}

/// Numbers taken from 31 330 real runs. A nudge that fires on output the
/// generic pass already handled is noise, and noise is how a channel stops
/// being read at all — including on the runs where it is right.
#[test]
fn the_compactor_nudge_only_fires_where_something_is_left_to_take() {
    use crate::wrap::nudge_worth_suggesting as worth;
    // The generic pass already did the job.
    assert!(!worth(636, 94), "seq: 77% saved, 94 tokens left");
    assert!(!worth(22920, 1913), "web: 92% saved");
    assert!(!worth(35950, 1395), "html: 96% saved");
    assert!(!worth(4208, 430), "skeleton: 90% saved");
    // A small residue is not worth a compactor however poorly it compacted.
    assert!(!worth(462, 186), "ls: only 186 tokens reach the model");
    assert!(!worth(1064, 579), "cat: content the caller asked for");
    // A real gap: little taken, substantial payload still shipped.
    assert!(worth(1260, 1131), "cargo: 13% saved, 1131 left");
    // Inflation is the strongest case of all.
    assert!(worth(1000, 1400), "the generic pass made it worse");
    // Degenerate input neither divides by zero nor fires.
    assert!(!worth(0, 0));
}

/// A `count` that never varies reports the size of the caller's own window,
/// not a property of the repository. And a count that fills the candidate pool
/// is a FLOOR — saying so keeps it from being read as a total.
#[test]
fn semantic_count_reports_what_ranking_retained_not_the_window() {
    use crate::code_intel::{output_mode_body_semantic_with_total, OutputMode};
    let hits: Vec<tokenade_core::semantic::Hit> = Vec::new();
    assert_eq!(
        output_mode_body_semantic_with_total(OutputMode::Count, &hits, Some((3, false))),
        "3\n",
        "an unsaturated count is exact"
    );
    assert_eq!(
        output_mode_body_semantic_with_total(OutputMode::Count, &hits, Some((40, true))),
        "40+\n",
        "a saturated count must read as a floor"
    );
    // No total available → the old behaviour, unchanged.
    assert_eq!(
        output_mode_body_semantic_with_total(OutputMode::Count, &hits, None),
        "0\n"
    );
}

/// Every shape an MCP server puts text in, checked against a server built to
/// leak. The previous pass walked `result.content[].text` and nothing else,
/// which is one of at least five places — a credential came back in clear
/// through an embedded `resource`, through `structuredContent`, through an
/// `isError` result and through `resources/read`, four different shapes with
/// one missing guard each. MCP is wired once and then wraps EVERY tool result,
/// so a gap here is a gap on every server a user has configured.
#[test]
fn mcp_responses_are_redacted_in_every_shape_a_server_uses() {
    const SEC: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    const PEM_BODY: &str = "MIIEowIBAAKCAQEAy8Dbv8prpJ";
    let pem = format!(
        "-----BEGIN RSA PRIVATE KEY-----\n{PEM_BODY}/0kKhlGeJYozo2t60EG8L0561g\n-----END RSA PRIVATE KEY-----"
    );
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "content text",
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"text","text":"aws_secret_access_key = {SEC}"}}]}}}}"#
            ),
            SEC,
        ),
        (
            "content text pem",
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"text","text":{:?}}}]}}}}"#,
                pem
            ),
            PEM_BODY,
        ),
        (
            "embedded resource",
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"resource","resource":{{"uri":"file:///a","text":"aws_secret_access_key = {SEC}"}}}}]}}}}"#
            ),
            SEC,
        ),
        (
            "structured output",
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"text","text":"ok"}}],"structuredContent":{{"creds":{{"aws_secret_access_key":"{SEC}"}}}}}}}}"#
            ),
            SEC,
        ),
        (
            "tool error result",
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"isError":true,"content":[{{"type":"text","text":"failed: aws_secret_access_key = {SEC}"}}]}}}}"#
            ),
            SEC,
        ),
        (
            "resources/read contents",
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"contents":[{{"uri":"file:///a","text":"aws_secret_access_key = {SEC}"}}]}}}}"#
            ),
            SEC,
        ),
        (
            "jsonrpc error message",
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"error":{{"code":-32000,"message":"upstream rejected aws_secret_access_key = {SEC}"}}}}"#
            ),
            SEC,
        ),
    ];

    let mut leaks = Vec::new();
    for (what, line, needle) in cases {
        let out = transform_mcp_response_line(&line, "fake", Some("t"), None);
        if out.contains(needle) {
            leaks.push(format!("{what}: {}", &out[..out.len().min(160)]));
        }
    }
    assert!(
        leaks.is_empty(),
        "a credential survived the MCP proxy ({} shape(s)):\n{}",
        leaks.len(),
        leaks.join("\n")
    );
}

/// The other half of the contract: a response that holds no secret must come
/// back byte-identical, and the base64 payload of an image must not be walked.
#[test]
fn mcp_redaction_leaves_an_ordinary_response_alone() {
    let clean = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"3 files changed"}]}}"#;
    let out = transform_mcp_response_line(clean, "fake", Some("t"), None);
    assert!(out.contains("3 files changed"), "content altered:\n{out}");

    let img = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"image","mimeType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk"}]}}"#;
    let out = transform_mcp_response_line(img, "fake", Some("t"), None);
    assert!(
        out.contains("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk"),
        "an image payload was rewritten:\n{out}"
    );
}

#[test]
fn a_fold_footer_points_at_the_stash_that_holds_its_lines() {
    let shown = format!(
        "pkg 1.0\n[tokenade folded 2082 line(s) total — {}]",
        tokenade_core::elision::fold_footer_promise()
    );
    let out = crate::wrap::point_footer_at_stash(&shown, "7332a803fa59");
    assert_eq!(
        out,
        "pkg 1.0\n[tokenade folded 2082 line(s) total — every folded line is in the stash `7332a803fa59` below]"
    );
}

#[test]
fn a_fold_that_could_not_be_stashed_stops_promising_a_hash() {
    // Disk full, read-only cache, a stash path that is not a directory: the
    // stash write fails, no banner is emitted, and the fold footer is left
    // telling the reader to run `expand-ref <hash>` with "the hash printed in
    // this output's banner". OBSERVED end to end: 2 322 lines folded away and
    // the only named recovery pointing at nothing.
    // Built from the SAME const the emitter uses, not a hand-copied string.
    // This test used to hold its own copy of the sentence, so rewording the
    // real footer would have left it green while `strip_unkept_hash_promise`
    // — an exact string match — quietly matched nothing. Three copies of one
    // sentence is three chances to drift.
    let shown = format!(
        "[tokenade folded 2322 line(s) total — {} `tokenade raw <cmd>`]",
        tokenade_core::elision::FOLD_FOOTER_HASH_PROMISE
    );
    let out = crate::wrap::strip_unkept_hash_promise(&shown);
    assert!(
        !out.contains("expand-ref"),
        "still names a hash nobody wrote: {out}"
    );
    assert!(
        out.contains("tokenade raw <cmd>"),
        "the recovery that DOES work was dropped too: {out}"
    );
    assert!(
        out.contains("2322"),
        "the count of what was folded was lost: {out}"
    );
}

#[test]
fn gain_accepts_the_recent_window_and_rejects_what_it_does_not_know() {
    // `--recent` joins the flag set; a typo must still be refused rather than
    // silently ignored, which is what the flag-validation sweep exists for.
    let ok = vec!["--recent".to_string()];
    assert!(super::validate_simple_bool_flags(
        &ok,
        "gain",
        &["--history", "--by-source", "--json", "--recent"]
    )
    .is_ok());
    let typo = vec!["--recnt".to_string()];
    assert!(
        super::validate_simple_bool_flags(
            &typo,
            "gain",
            &["--history", "--by-source", "--json", "--recent"]
        )
        .is_err(),
        "an unknown flag was accepted silently"
    );
}

// ── `gain` per-op figures: say when one day owns the number ────────────────
// The percentage beside an op is a volume-weighted SUM over the window, so a
// single busy day decides it. Measured on a real install: `html-to-markdown`
// read −57 % over seven days while 91 % of that volume was ONE day scoring
// −56 %, and the two most recent days scored −90 % and −93 %. The figure was
// read as a trend; it was a one-day snapshot, of the OLDEST day in the window.
#[test]
fn an_op_whose_window_is_one_busy_day_says_so() {
    use crate::dashboard_cli::heaviest_day_note;
    let days =
        |v: &[(i64, u64)]| -> std::collections::BTreeMap<i64, u64> { v.iter().copied().collect() };

    // 91 % on day 0 (1970-01-01), 20 calls, everything dated → named.
    let d = days(&[(0, 910), (1, 90)]);
    assert_eq!(
        heaviest_day_note(Some(&d), 1000, 20),
        "  (91% of it on 01-01)",
        "the share must not read as a second saving rate"
    );

    // Spread across days: no single one owns it, so no note.
    let d = days(&[(0, 400), (1, 300), (2, 300)]);
    assert_eq!(heaviest_day_note(Some(&d), 1000, 20), "");

    // Concentrated but only 4 calls — the `N×` column already said it.
    let d = days(&[(0, 910), (1, 90)]);
    assert_eq!(heaviest_day_note(Some(&d), 1000, 4), "");

    // Half the volume predates the `ts` field: concentration is NOT knowable,
    // so nothing is claimed rather than claimed from the dated half.
    let d = days(&[(0, 500)]);
    assert_eq!(heaviest_day_note(Some(&d), 1000, 20), "");

    // No day data at all.
    assert_eq!(heaviest_day_note(None, 1000, 20), "");
}

// ── `gain`'s est. bucket holds two different confidences ───────────────────
// It was printed as one line reading "inferred baseline, not measured". On a
// real install 73 % of that bucket is `skeleton`, where the file WAS read and
// tokenised and only the alternative is assumed; the other 27 % has no input
// at all and its baseline is a published third-party ratio. The confident half
// was the larger one, and the line disowned it.
#[test]
fn a_measured_input_is_not_called_a_borrowed_ratio() {
    use crate::dashboard_cli::baseline_is_borrowed;
    // skeleton: real file in, skeleton out — measured input.
    assert!(!baseline_is_borrowed(None, "skeleton", 9000, 400));
    // Writer already expanded it from the constant and said so.
    assert!(baseline_is_borrowed(Some("ci_heuristic"), "structure_map", 2326, 1000));
    // Arrived unmeasurable; the reader multiplies it, so the baseline is ours.
    assert!(baseline_is_borrowed(None, "whatever", 1000, 1000));
    // An unrelated class with a real input stays measured.
    assert!(!baseline_is_borrowed(Some("overhead"), "auto:json", 9000, 400));
    // A counterfactual multiplier, stamped at write time.
    assert!(baseline_is_borrowed(Some("counterfactual"), "feed", 1300, 1000));
    // And the same, on a line written BEFORE the stamp existed: the op is the
    // only evidence left, and `web_search_credit_savings` is its one writer.
    assert!(baseline_is_borrowed(None, "web-search", 3000, 1000));
}

#[test]
fn truncated_map_envelope_stays_parseable_and_names_its_recovery() {
    // `map --json` had no ceiling of its own. MEASURED in production: one call
    // at `/home/username` delivered 669 829 tokens — more than most sessions
    // can hold. The replacement must not fix that by breaking `--json`: a
    // script piping this into `jq` still gets an object with the counts.
    let e = crate::code_intel::truncated_map_envelope(
        "/home/u/proj",
        "unknown",
        1341,
        21817,
        669_829,
        "c51e1fc17925",
        "/tmp/c51e1fc17925.json",
    );
    let v: serde_json::Value = serde_json::from_str(&e).expect("envelope must be valid JSON");
    assert_eq!(v["files_indexed"], 1341, "counts survive the truncation");
    assert_eq!(v["symbols"], 21817);
    assert_eq!(
        v["truncated"], true,
        "a truncation that does not say so is a lie"
    );
    // The whole point: the omitted document stays reachable, by name.
    assert_eq!(v["full_output_ref"], "c51e1fc17925");
    let hint = v["hint"].as_str().unwrap();
    assert!(
        hint.contains("expand-ref c51e1fc17925"),
        "the recovery must be spelled out, not implied: {hint}"
    );
}

/// `uninstall` deletes trees, so where it thinks HOME is matters more than
/// anywhere else in the product.
///
/// MEASURED 2026-08-04 on the release binary: with `HOME=.` the dry-run offered
/// `would delete ./.tokenade` — a directory in whatever the CWD happened to be.
/// `HOME=""` had already been made to refuse after the 0.9.5 data-loss
/// incident; a RELATIVE home reached the same place by another route.
#[test]
fn uninstall_refuses_a_relative_home_and_says_so() {
    use crate::install::remove_tokenade_data;
    use std::path::Path;
    for rel in [".", "..", "some/relative/dir"] {
        let out = remove_tokenade_data(Some(Path::new(rel)), None, true);
        assert!(
            !out.iter().any(|l| l.contains("would delete")),
            "a relative HOME must not name a deletion target: {out:?}"
        );
        assert!(
            out.iter().any(|l| l.contains("could not locate")),
            "and the refusal must be VISIBLE — a silent no-op reads as success: {out:?}"
        );
    }
    // An absolute home is still handled exactly as before.
    let tmp = std::env::temp_dir().join(format!("tokenade-uninst-{}", std::process::id()));
    let _ = std::fs::create_dir_all(tmp.join(".tokenade"));
    let out = remove_tokenade_data(Some(&tmp), None, true);
    assert!(
        out.iter().any(|l| l.contains("would delete")),
        "absolute home must still list its target: {out:?}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The hint is advice tokenade injects on EVERY qualifying edit, so a command
/// that answers with nothing costs a turn each time.
///
/// MEASURED 2026-08-04 on this install's own 737 emitted hints: 354 named a
/// file untracked in its repo, 22 a file in no repo — `git diff` was a no-op
/// or an error on 51 % of them. The precondition the hint has already checked
/// by this point (a whole-file dedup snapshot exists) is exactly what makes a
/// re-Read come back as a diff, for tracked and untracked files alike.
#[test]
fn the_edit_hint_never_names_a_command_that_needs_git() {
    let hint = crate::edit_hint::edit_hint_text("/nowhere/untracked.rs", 4321);
    assert!(
        !hint.contains("git "),
        "sends the agent to git, which says nothing about an untracked file: {hint}"
    );
    assert!(
        hint.contains("Read it again"),
        "no longer names the path that works without git: {hint}"
    );
    // The cost figure is what makes the nudge concrete; keep it.
    assert!(hint.contains("4321"), "dropped the token estimate: {hint}");
    assert!(
        hint.contains("/nowhere/untracked.rs"),
        "dropped the path: {hint}"
    );
}

/// The fail-open boundary only holds if EVERY path into compaction goes through
/// it. A fourth call site added later would silently reopen the hole this
/// closed, and nothing at runtime would say so — the failure only shows up as a
/// user losing a command's output.
#[test]
fn no_channel_reaches_a_compactor_without_the_fail_open_boundary() {
    // EVERY channel, which is what the doc above always claimed and what this
    // scan did not do: it read `wrap.rs` alone while `hookwrap.rs` — the
    // highest-traffic compaction path in the product — held two bare calls and
    // `main.rs` a third. A guard narrower than its own promise reads as
    // coverage.
    //
    // Every file of each channel: a call site that moved into a submodule the
    // scan does not name is a call site this guard stops seeing.
    let cli_src = tokenade_core::source_guard::crates_dir()
        .join("tokenade-cli")
        .join("src");
    let src = format!(
        "{}{}{}",
        tokenade_core::source_guard::module_family(&cli_src, "wrap"),
        tokenade_core::source_guard::module_family(&cli_src, "hookwrap"),
        tokenade_core::source_guard::module_family(&cli_src, "main"),
    );
    let src = src.as_str();
    let bare = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("filter_command(") && !l.contains("filter_command_fail_open("))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .map(|(i, l)| format!("{}: {}", i + 1, l.trim()))
        .collect::<Vec<_>>();
    assert!(
        bare.is_empty(),
        "a channel calls the compactor without the panic boundary:\n{}",
        bare.join("\n")
    );
}
