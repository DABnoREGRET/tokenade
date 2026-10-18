//! End-to-end integration tests.
//!
//! These spawn the real release binary so the whole stack — argument
//! parsing, file I/O, hookwrap pipeline, MCP framing, gain ledger
//! writes — is exercised. Unit tests cover individual modules;
//! these catch wiring bugs between them.
//!
//! Each test runs in its own temp HOME so the suite is hermetic and
//! parallel-safe. We set `HOME=<tmp>` and `USERPROFILE=<tmp>` (the
//! Windows equivalent) on every child process — tokenade's
//! `platform::home_dir()` checks both.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// One file per subject, split out of a 4026-line `e2e.rs`. Still ONE test
// binary: extra files under `tests/` would each build and link their own.

mod bridges;
mod code_intel;
mod compat_matrix;
mod gain;
mod hookwrap;
mod install;
mod invariants;
mod misc;
mod read_upgrade;
mod web;
mod wiring;
mod wrap;

/// Locate the freshly-built release binary. We prefer `target/release`
/// over `~/.local/bin/tokenade` so a CI run uses the binary it just
/// produced, not a stale install.
fn binary() -> PathBuf {
    // The binary cargo built for THIS test run — always the DEBUG build. A
    // stray `target/release/tokenade` from a manual `cargo build --release`
    // may be stale. CARGO_BIN_EXE is Cargo's canonical path to the just-built
    // bin under test.
    PathBuf::from(env!("CARGO_BIN_EXE_tokenade"))
}

/// A scratch HOME directory removed when the guard drops. Combined
/// with passing HOME via env, this makes every test independent.
struct TempHome {
    dir: PathBuf,
}

impl TempHome {
    fn new(label: &str) -> Self {
        // Process-id keeps parallel suites from colliding.
        let dir = std::env::temp_dir().join(format!(
            "tokenade-e2e-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempHome { dir }
    }
    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run the binary with the given args + isolated HOME. Returns
/// (stdout, stderr, exit_code).
fn run(args: &[&str], home: &Path) -> (String, String, i32) {
    run_bin(&binary(), args, home)
}

/// Every spawn of the binary under test gets the SAME environment.
///
/// There were four copies of this list — `run_bin`, `run_with_stdin`,
/// `run_with_stdin_bytes`, `run_in` — and they had already drifted: only
/// `run_bin` set `TOKENADE_FORCE_GAIN`, so a test driving the binary through
/// stdin (which is every hook test) asserted against a ledger the dev build
/// never wrote to. Then 2026-08-08 added three `env_remove`s to `run_bin`
/// alone, which would have been the same drift a second time. One list, one
/// place.
/// The developer's already-downloaded embedding model, when it is there and
/// complete.
///
/// The containment below redirects `XDG_CACHE_HOME` at the per-test temp dir,
/// which is right — a test must not write into a real `~/.cache`. But the model
/// cache is keyed on exactly that, so every test that reaches retrieval
/// re-downloads **132 MB**. Measured 2026-08-27 while the CLI suite refused to
/// finish: `read_prompt_does_not_consume_the_path` sat at **541 seconds** doing
/// precisely that, one child process, no CPU, one socket open.
///
/// `TOKENADE_MODEL_DIR` is READ-ONLY to `model_fetch` — it checks the files are
/// there and mmaps them — so containment is untouched: nothing is written
/// outside the temp HOME. Pinned to `MODEL_VERSION` rather than globbed, so a
/// version bump makes the tests download the new model instead of silently
/// exercising the old one. When the host has no model the variable stays unset
/// and the test downloads exactly as before.
fn host_model_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let d = home
        .join(".cache")
        .join("tokenade")
        .join("model")
        .join(tokenade_core::model_fetch::MODEL_VERSION);
    d.join("model.safetensors").is_file().then_some(d)
}

fn e2e_env<'a>(cmd: &'a mut Command, home: &Path) -> &'a mut Command {
    if let Some(d) = host_model_dir() {
        cmd.env("TOKENADE_MODEL_DIR", d);
    }
    cmd.env_remove("TOKENADE_DEBUG_LOG")
        // Ambient tuning knobs must NOT reach the binary under test. A
        // developer's shell (and the tokenade hook itself) sets
        // `TOKENADE_PRESSURE`, which HALVES the disclosure threshold at 0.5 —
        // so a test whose fixture sits between the two thresholds passes
        // locally and fails in CI, or the reverse. Caught 2026-08-08:
        // `e2e_wrap_credits_the_catch_all_fold_exactly_once` failed in one
        // workspace run and passed in the next, and the only difference was
        // `TOKENADE_PRESSURE=0.50` in the environment.
        .env_remove("TOKENADE_PRESSURE")
        .env_remove("TOKENADE_DISCLOSE_THRESHOLD")
        .env_remove("TOKENADE_READ_DISCLOSE_THRESHOLD")
        // The SESSION identity, for the same reason and with a sharper edge.
        //
        // `read` folds a repeat to a `§ref:` only when the session asking
        // provably received those bytes, so two invocations that share a
        // session key are deliberately NOT independent. A developer's shell
        // exports `TOKENADE_SESSION` — the tokenade hook itself sets it — and
        // it reached the binary under test.
        //
        // Measured 2026-08-20: `read_empty_prompt_falls_back_to_the_full_serve`
        // compares `read <doc>` against `read <doc> --prompt "   "` and expects
        // the same bytes. Under an inherited session the second call came back
        // `§ref:15f1f7ac631b§ (~770 tokens …, identical to a prior call)`, and
        // the test read a working dedup as a broken fallback. It was invisible
        // until this session gave the dedup layer the session key it had always
        // been missing — the variable was there all along and did nothing.
        .env_remove("TOKENADE_SESSION")
        // Its fallback too: Claude Code exports it to every command, this
        // test run's included (`session::ambient_session_key`).
        .env_remove("CLAUDE_CODE_SESSION_ID")
        // `shell-init` without `--shell` reads `$SHELL`. A developer on fish
        // (this host) then gets the fish fragment as the "default", so
        // `e2e_shell_init_refuses_what_it_does_not_understand` compared
        // `--shell fish` to itself and failed. Empty → POSIX fallback.
        .env_remove("SHELL")
        .env("HOME", home)
        .env("USERPROFILE", home)
        // The XDG family too, and it is not belt-and-braces. `platform`'s
        // ambient-override gate honours `$XDG_CONFIG_HOME` whenever the home it
        // is handed IS the process home — which is precisely the case here,
        // since HOME is set to the temp dir above. So a developer who exports
        // `XDG_CONFIG_HOME` would have these tests write into their REAL
        // `~/.config`. It happens not to be set on the machine this was found
        // on (2026-08-08), which is the only reason containment held: a
        // precaution that guards HOME while a sibling variable overrides the
        // result is the same HOME-containment defect one layer down.
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CACHE_HOME")
        // `ambient_override` is called with FIVE variable names, not three:
        // the agent-specific roots sit under the identical gate, and
        // `agent_transcripts` registers the parsers that read them, so
        // `dashboard` reaches them with the temp HOME. None of the four is set
        // on this machine either — the same accident, one family over. A
        // usage assertion would otherwise read the developer's real Goose or
        // Crush history and call it the fixture's.
        .env_remove("GOOSE_PATH_ROOT")
        .env_remove("CRUSH_GLOBAL_DATA")
        .env_remove("ZS_DATA_DIR")
        .env_remove("ANTIGRAVITY_DATA_DIR")
        // No test may open a browser. `login` calls `xdg-open`, which launches
        // a real Chrome that DETACHES: killing the tokenade child cannot reap
        // a grandchild spawned to outlive it. Measured 2026-08-08 — 92 orphan
        // `chrome_crashpad_handler` processes, the oldest alive 51 hours, one
        // batch per suite run. `SSH_CONNECTION` forces `is_headless()` on
        // every platform (the only reader of it in the product), so this also
        // holds on macOS, where there is no DISPLAY to be missing.
        .env("SSH_CONNECTION", "10.0.0.1 51000 10.0.0.2 22")
        // cwd defaults to the temp HOME, and that is not cosmetic: several
        // commands (`install`, the SessionStart hook) scaffold their CWD, and
        // cargo hands a test binary the CRATE DIRECTORY. Every such spawn was
        // writing `CLAUDE.md` + `.tokenade/scaffold.flag` into the working
        // repository — one of them git-TRACKED. Setting HOME made it
        // look contained; HOME and cwd are two different things. `run_in`
        // overrides this when a test needs a specific project root.
        .current_dir(home)
        // Quiet down the cargo build hook chatter — we're testing the binary,
        // not the agent harness.
        .env("TOKENADE_HOOK_DISABLED", "1")
        // The binary under test is a DEV build, which books no gains (see
        // `gain::record_inner`). e2e asserts the SHIPPED behaviour, so it opts
        // back in — otherwise every ledger-shape test reads an absent file.
        .env("TOKENADE_FORCE_GAIN", "1")
}

/// Byte-exact twin of `run`. Every other runner here decodes with
/// `from_utf8_lossy`, which is precisely the transformation the byte-fidelity
/// tests exist to detect — asserting through it would compare our substitution
/// against itself and pass in both worlds.
#[allow(dead_code)]
fn run_bytes(args: &[&str], home: &Path) -> (Vec<u8>, Vec<u8>, i32) {
    let out = tokenade_core::process_util::retry_while_text_file_busy(|| {
        let mut cmd = Command::new(binary());
        cmd.args(args);
        e2e_env(&mut cmd, home).output()
    })
    .expect("spawn tokenade binary");
    (out.stdout, out.stderr, out.status.code().unwrap_or(-1))
}

/// [`run`], plus an explicit session identity.
///
/// `e2e_env` REMOVES `TOKENADE_SESSION` on purpose — a developer's exported
/// session must never make two invocations share dedup state. The web REPLAY
/// layer is keyed on exactly that variable and returns early without it, so the
/// removal also meant no end-to-end test could reach it, and none did: a batch
/// route that neither folded nor remembered a delivered page shipped
/// unobserved. Handing the identity back, per test and by name, is the opt-in;
/// the removal stays the default. One env list still, one place.
fn run_with_session(args: &[&str], home: &Path, session: &str) -> (String, String, i32) {
    run_with_env(args, home, &[("TOKENADE_SESSION", session)])
}

/// [`run`], with named variables handed back after `e2e_env` has stripped them.
///
/// The strip list is the default and stays the default; this is the door for a
/// test that needs one of those knobs to be the thing under test — a session
/// identity, or a disclosure threshold small enough to reach a gate without
/// building a ninety-page fixture to get there. Still ONE env list, in one
/// place, with the overrides applied after it.
fn run_with_env(args: &[&str], home: &Path, envs: &[(&str, &str)]) -> (String, String, i32) {
    let out = tokenade_core::process_util::retry_while_text_file_busy(|| {
        let mut cmd = Command::new(binary());
        cmd.args(args);
        e2e_env(&mut cmd, home);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.output()
    })
    .expect("spawn tokenade binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn run_bin(bin: &Path, args: &[&str], home: &Path) -> (String, String, i32) {
    // ETXTBSY: callers (`run_copied`) hand us a binary they just wrote, and a
    // sibling test thread that forks in that window inherits the write fd, so
    // the kernel refuses the exec. Same class as the updater's post-download
    // probe — one shared helper. See `retry_while_text_file_busy`.
    let out = tokenade_core::process_util::retry_while_text_file_busy(|| {
        let mut cmd = Command::new(bin);
        cmd.args(args);
        e2e_env(&mut cmd, home).output()
    })
    .expect("spawn tokenade binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Run a THROWAWAY COPY of the binary (inside `home`, well away from any path
/// `uninstall` touches) so destructive commands — `uninstall` wipes ~/.tokenade
/// AND self-deletes the running binary — exercise their full real behaviour
/// without ever touching the shared build binary the rest of the suite spawns.
/// Re-copies fresh on every call, so a self-deleting command can be invoked
/// repeatedly (e.g. idempotency checks). This makes the uninstall e2e test
/// hermetic instead of relying on the production build-artifact guard.
fn run_copied(args: &[&str], home: &Path) -> (String, String, i32) {
    let dir = home.join(".tokenade-testbin");
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join(if cfg!(windows) {
        "tokenade.exe"
    } else {
        "tokenade"
    });
    // Unlink before copying — the same fix `run_copied_active` already carries,
    // and for the same reason: `install` leaves a child (the llm-proxy) holding
    // this executable open, so a second call against the same home —
    // install-then-uninstall — hit `ETXTBSY "Text file busy"` writing over the
    // binary it had just run. Removing the directory entry first gives the copy
    // a fresh inode and leaves the still-running child on the old one. Every
    // existing caller invoked this once, which is why the race had never shown.
    //
    // NOT a unique filename per call, which was the first thing I reached for:
    // this helper does a real `fs::copy` of a 625 MB debug binary, so a new name
    // each time turns one test's four calls into 2,5 GB — the `StorageFull`
    // failure the neighbour's comment records from an 8 GB tmpfs.
    let _ = std::fs::remove_file(&dst);
    std::fs::copy(binary(), &dst).expect("copy test binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    run_bin(&dst, args, home)
}

/// Like `run_copied` but with tokenade ACTIVE (no `TOKENADE_HOOK_DISABLED`),
/// which real users always are — needed to exercise the post-upgrade
/// self-heal/refresh path (the flag is a bench/CI control that suppresses it).
/// The one copy of the test binary this process makes, outside `target/`.
///
/// Named by pid so parallel suites never share it; siblings whose process is
/// gone are swept on the way in, because nothing else would ever remove them.
fn shared_binary() -> &'static Path {
    static SHARED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| {
        let root = std::env::temp_dir();
        // This loop was here first and was right; it only ever knew ONE
        // prefix. The shared sweep is the same rule over the whole
        // `tokenade-*` family, so adopting it here also clears the 235 other
        // helpers that never learned to clean up.
        tokenade_core::testtmp::sweep_stale_scratch_dirs();
        let dir = root.join(format!("tokenade-e2e-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dst = dir.join(if cfg!(windows) {
            "tokenade.exe"
        } else {
            "tokenade"
        });
        if !dst.exists() {
            std::fs::copy(binary(), &dst).expect("copy test binary");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        dst
    })
}

fn run_copied_active(args: &[&str], home: &Path) -> (String, String, i32) {
    let dir = home.join(".tokenade-testbin");
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join(if cfg!(windows) {
        "tokenade.exe"
    } else {
        "tokenade"
    });
    // Unlink before copying: a test that invokes this helper TWICE against the
    // same home (e.g. "consume the post-upgrade marker, then plant a stale
    // shim") would otherwise hit ETXTBSY writing over the binary it just ran.
    // Removing the directory entry first gives the copy a fresh inode and
    // leaves any still-running child on the old one.
    let _ = std::fs::remove_file(&dst);
    // ONE copy per process, hardlinked into each test's HOME. The copy exists
    // only so the binary runs from OUTSIDE `target/` (the post-upgrade refresh
    // is build-dir gated) — nothing ever writes to it, so a shared inode is
    // safe, and hardlinking cannot raise ETXTBSY the way opening-for-write can.
    // Measured 2026-08-27: the debug binary is 625 631 144 bytes and this
    // helper is called six times, so the suite wrote 3,75 GB into an 8 GB
    // tmpfs every run. With less than ~4 GB free it failed as
    // `StorageFull` inside four shim tests, which reads exactly like a shim
    // bug and is not one.
    match std::fs::hard_link(shared_binary(), &dst) {
        Ok(()) => {}
        // Different filesystem (a TMPDIR elsewhere) — fall back to the copy.
        Err(_) => {
            std::fs::copy(binary(), &dst).expect("copy test binary");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // Fully isolated env: the suite may run under `tokenade wrap` (which
    // exports TOKENADE_WRAPPING / *_HOOK_DISABLED etc.), any of which would
    // suppress the very post-upgrade self-heal we're testing. Clear everything
    // and hand back only PATH + HOME so the binary behaves like a real user's.
    let path = std::env::var_os("PATH").unwrap_or_default();
    // Exec'ing a binary we just wrote is racy here: a sibling test thread that
    // forks inherits our still-open write fd and the kernel returns ETXTBSY.
    // Same class as the updater's post-download probe — one shared helper.
    let out = tokenade_core::process_util::retry_while_text_file_busy(|| {
        Command::new(&dst)
            .args(args)
            .env_clear()
            .env("PATH", &path)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .output()
    })
    .expect("spawn tokenade binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Send `stdin_payload` to the binary, return its stdout.
fn run_with_stdin(args: &[&str], home: &Path, stdin_payload: &str) -> (String, String, i32) {
    let bin = binary();
    let mut c = Command::new(&bin);
    c.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = e2e_env(&mut c, home).spawn().expect("spawn tokenade");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Same as `run_with_stdin` but the payload is raw bytes — so we can feed a
/// compactor genuinely invalid UTF-8 (Latin-1 / cp1252 / UTF-16-ish), which a
/// `&str` payload cannot represent.
fn run_with_stdin_bytes(args: &[&str], home: &Path, payload: &[u8]) -> (String, String, i32) {
    use std::io::Write;
    let bin = binary();
    let mut c = Command::new(&bin);
    c.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = e2e_env(&mut c, home).spawn().expect("spawn tokenade");
    child.stdin.as_mut().unwrap().write_all(payload).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Like `run`, but also pins the child's working directory — needed for
/// the index-backed exploration commands (`query`, `map`) which resolve
/// the project root from CWD.
fn run_in(args: &[&str], home: &Path, cwd: &Path) -> (String, String, i32) {
    let bin = binary();
    let mut c = Command::new(&bin);
    c.args(args);
    // `current_dir` AFTER `e2e_env`, which ends by setting it to `home`.
    // Setting it first meant the override won and this helper ran every one of
    // its 33 callers in the temp HOME instead of the project root they asked
    // for — the comment inside `e2e_env` says `run_in` overrides it, and the
    // order made that false. Tests still passed because HOME CONTAINS the
    // project directory in every fixture, so a root walked from HOME indexes
    // the fixture's files anyway; what it does not do is put artefacts where
    // the caller looks for them.
    let out = e2e_env(&mut c, home)
        .current_dir(cwd)
        .output()
        .expect("spawn tokenade binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// A long document with one section that answers a question the rest does not.
fn haystack_doc() -> String {
    let mut s = String::from("# Handbook\n\n");
    for i in 0..40 {
        s.push_str(&format!(
            "## Chapter about topic {i}\n\nThis chapter discusses subject matter number {i} \
             at some length, with sentences that carry no operational detail whatsoever \
             and exist to pad the document so retrieval has something to sift.\n\n"
        ));
    }
    s.push_str(
        "## Rotating the signing certificate\n\nRun `handbook rotate --force` and then \
         restart the daemon. The rotation interval is ninety days.\n\n",
    );
    for i in 40..80 {
        s.push_str(&format!(
            "## Chapter about topic {i}\n\nMore padding prose for subject {i}, again with \
             nothing operational in it at all, purely to make the document long.\n\n"
        ));
    }
    s
}

/// A minimal but real .docx: the format is a ZIP, so reading it as text
/// destroys it before any detector runs.
fn tiny_docx(body_text: &str) -> Vec<u8> {
    use std::io::Write as _;
    let doc = format!(
        r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{body_text}</w:t></w:r></w:p></w:body></w:document>"#
    );
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let o: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        z.start_file("[Content_Types].xml", o).unwrap();
        z.write_all(br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap();
        z.start_file("word/document.xml", o).unwrap();
        z.write_all(doc.as_bytes()).unwrap();
        z.finish().unwrap();
    }
    buf
}

/// Build a minimal but REAL file of each family. Hand-rolled on purpose: the
/// point is to exercise the sniffer, and a fixture that is only plausible to a
/// human proves nothing about it.
fn binary_samples() -> Vec<(&'static str, Vec<u8>)> {
    let mut v: Vec<(&'static str, Vec<u8>)> = Vec::new();
    // gzip: header + a deflate stream of a few bytes.
    v.push((
        "tiny.gz",
        vec![
            0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0, 3, 0x4b, 0x4c, 0x4a, 0x06, 0x00, 0xc2, 0x41,
            0x24, 0x35, 0x03, 0x00, 0x00, 0x00,
        ],
    ));
    // PNM: tiny but unmistakably an image.
    let mut pnm = b"P6\n2 2\n255\n".to_vec();
    pnm.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
    v.push(("t.pnm", pnm));
    // ELF, WASM, Java class, TrueType: magic + filler.
    for (name, magic) in [
        ("a.so", &b"\x7fELF\x02\x01\x01\x00"[..]),
        ("a.wasm", &b"\x00asm\x01\x00\x00\x00"[..]),
        ("a.class", &b"\xca\xfe\xba\xbe\x00\x00\x00\x34"[..]),
        ("a.ttf", &b"\x00\x01\x00\x00"[..]),
    ] {
        let mut b = magic.to_vec();
        b.extend((0u16..600).map(|i| (i % 256) as u8));
        v.push((name, b));
    }
    v
}

/// Every secret shape, through every command that hands text back.
///
/// This contract — "a credential never reaches the model in clear" — has been
/// broken three separate times, each on a DIFFERENT path: a compacted stdout
/// cap that cut a secret in two before redaction ran, an `Unknown` passthrough
/// that skipped the print-site scrub, and `--prompt` extraction that read the
/// document before it was redacted. Each fix was correct and each left the next
/// path unguarded, because the guard is applied per call site rather than by
/// construction.
///
/// So the gate is the user-visible contract, exercised end to end on the real
/// binary rather than on any one function: plant the secret, run the command,
/// assert the secret is not in what came back.
fn secret_shapes() -> Vec<(&'static str, String, &'static str)> {
    // (label, the text to plant, the exact substring that must never come back)
    let aws_secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let pem = "-----BEGIN RSA PRIVATE KEY-----\n\
               MIIEowIBAAKCAQEAy8Dbv8prpJ/0kKhlGeJYozo2t60EG8L0561g13R29LvMR5hy\n\
               vGZlGJpmn65+A4xHXInJYiPuKzrKUnApeLZ+vw1HocOAZtWK0z3r26uA8kQYOKX9\n\
               -----END RSA PRIVATE KEY-----";
    vec![
        (
            "aws",
            format!(
                "aws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = {aws_secret}\n"
            ),
            aws_secret,
        ),
        (
            "pem_multiline",
            format!("# deploy key\n{pem}\n# end\n"),
            "MIIEowIBAAKCAQEAy8Dbv8prpJ",
        ),
        (
            "github_token",
            "GITHUB_TOKEN=ghp_1234567890abcdefghijklmnopqrstuvwx\n".to_string(),
            "ghp_1234567890abcdefghijklmnopqrstuvwx",
        ),
        (
            "password_assignment",
            "DB_PASSWORD=hunter2CorrectHorseBatteryStaple\nDB_HOST=db.internal\n".to_string(),
            "hunter2CorrectHorseBatteryStaple",
        ),
    ]
}

/// The shapes above, wrapped in each file format the dispatcher recognises: a
/// compactor only leaks on the path it actually takes, and the path is chosen
/// by the shape of the file.
fn planted_documents(secret_line: &str) -> Vec<(&'static str, String)> {
    vec![
        ("env", format!("PATH=/usr/bin\n{secret_line}\nHOME=/root\n")),
        (
            "json",
            format!(
                "{{\n  \"service\": \"api\",\n  \"note\": {:?},\n  \"replicas\": 3\n}}\n",
                secret_line.trim()
            ),
        ),
        (
            "yaml",
            format!("service: api\nnote: |\n  {}\nreplicas: 3\n", secret_line.trim()),
        ),
        (
            "log",
            format!("2024-01-01T00:00:00Z INFO boot\n2024-01-01T00:00:01Z INFO {}\n", secret_line.trim()),
        ),
        (
            "md",
            format!("# Runbook\n\nSet the credential:\n\n```sh\n{}\n```\n\n- step two\n", secret_line.trim()),
        ),
        // Buried in a large document, which is where the bug this test was
        // written for lived: every compactor CLIPS long values and clipping
        // runs BEFORE redaction, so a rule that needs a closing marker never
        // sees one. A small fixture can never reach that path.
        (
            "json_large",
            {
                let mut rows = String::new();
                for i in 0..400 {
                    rows.push_str(&format!(
                        "    {{ \"id\": {i}, \"name\": \"service-{i}\", \"region\": \"eu-west-{}\" }},\n",
                        i % 3
                    ));
                }
                format!(
                    "{{\n  \"items\": [\n{rows}    {{ \"id\": 999, \"name\": \"last\" }}\n  ],\n  \"note\": {:?}\n}}\n",
                    secret_line.trim()
                )
            },
        ),
        // Source files: a hardcoded credential in code is the single most
        // common way one ends up in a repository, and code takes a different
        // route through the CLI (skeleton/query/map) than a document does.
        (
            "rust_source",
            format!(
                "//! Service client.\npub struct Client;\n\nimpl Client {{\n    /// Connect.\n    pub fn new() -> Self {{\n        let _cfg = \"{}\";\n        Self\n    }}\n}}\n",
                secret_line.trim().replace('\\', "").replace('"', "'").replace('\n', " ")
            ),
        ),
        (
            "python_source",
            format!(
                "\"\"\"Service client.\"\"\"\n\n\nclass Client:\n    def __init__(self):\n        self.cfg = \"\"\"{}\"\"\"\n\n    def connect(self):\n        return self.cfg\n",
                secret_line.trim()
            ),
        ),
        (
            "log_large",
            {
                let mut lines = String::new();
                for i in 0..600 {
                    lines.push_str(&format!("2024-01-01T00:00:{:02}Z INFO worker {i} ok\n", i % 60));
                }
                format!("{lines}2024-01-01T01:00:00Z WARN {}\n", secret_line.trim())
            },
        ),
    ]
}

/// Drive a HOOK subcommand. Identical to `run_with_stdin` except for the one
/// thing that matters: it does NOT set `TOKENADE_HOOK_DISABLED`.
///
/// Every other runner in this file sets it — correctly, because they are
/// testing the CLI and the agent harness must not intercept their own child
/// processes. But the hook subcommands read that same variable and return
/// immediately when it is set, emitting nothing. A hook test written with
/// `run()` therefore passes without ever executing the code it names, and the
/// only symptom is an empty stdout that reads like "the hook decided not to
/// act". That cost an hour of chasing a leak probe that could never fire.
fn run_hook(args: &[&str], home: &Path, stdin_payload: &str) -> (String, String, i32) {
    run_hook_as(None, args, home, stdin_payload)
}

/// `run_hook` with the agent DECLARED rather than inferred.
///
/// The agents whose PostToolUse hook is the plain `hookwrap` subcommand
/// (codex, droid, grok) carry no per-agent entry point, so
/// attribution falls to `detect::from_env()`. A test that left it to inference
/// would be asserting against whatever spawned the test binary.
#[allow(dead_code)]
fn run_hook_as(
    agent: Option<&str>,
    args: &[&str],
    home: &Path,
    stdin_payload: &str,
) -> (String, String, i32) {
    let bin = binary();
    let mut c = Command::new(&bin);
    if let Some(a) = agent {
        c.env("TOKENADE_AGENT", a);
    }
    c.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The ONLY difference from `e2e_env`, and it is the whole point of this
    // helper: `TOKENADE_HOOK_DISABLED` mutes a hook's compaction, so a hook
    // driven through `run_with_stdin` exercises the kill-switch path and
    // nothing else.
    let mut child = e2e_env(&mut c, home)
        .env_remove("TOKENADE_HOOK_DISABLED")
        .spawn()
        .expect("spawn tokenade");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}
