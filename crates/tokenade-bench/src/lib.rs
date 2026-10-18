//! The end-to-end measurement bench.
//!
//! The other binaries of this crate measure the LIBRARY, in process
//! (`dispatch_named` & co). That is the right tool for "does this compactor
//! fold well?", and that is exactly why none of them ever ran into the traps
//! that cost **37 recorded instrument faults**: those traps are all
//! properties of the end-to-end CLI path.
//!
//! This module is the single place where that path is measured, so that those
//! traps are paid for once:
//!
//! 1. **The output never goes through my terminal.** An agent `Bash` goes
//!    through the hook, which compacts — I have already read my own compacted
//!    output believing it was the product's. Here it is `Command::output()`, so bytes.
//! 2. **Fresh state per bench** (`TOKENADE_CONFIG_DIR`, `TOKENADE_STASH_DIR`,
//!    `TOKENADE_DEDUP_DB`). Without it the second arm of an A/B sees a duplicate
//!    the first one just recorded, and wrongly reports "DIFFERS".
//! 3. **The real extension** per document: routing is done by extension, and
//!    probe files named `.dat` reach no compactor — a fuzz returned "0 hits"
//!    on a version we KNEW crashed.
//! 4. **The profile is declared**, and [`Run::ms_comparable`] rejects a time
//!    measured in debug (43× the gap with release, measured).
//! 5. **A signal is not a success**: [`Run::survived`] reads `code.is_some()`,
//!    because a test whose condition is "non-zero exit" accepts the OOM kill
//!    it was meant to prevent.
//! 6. **Mandatory canary**: [`conclude`] refuses to return "nothing found"
//!    until the probe has first reproduced a known defect.
//!
//! When a probe lies, the fix goes HERE, with a test. Never a workaround in
//! the iteration — that workaround is what happened 37 times.

pub mod docs_corpus;
pub mod mutate;
pub mod transcripts;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

// The end-to-end bench stays here. Four topics move out, and `pub use` puts them
// back at the `tokenade_bench::` path that the eleven binaries already spell.
mod captures;
mod delivery;
mod instrument;
mod report;
pub use captures::*;
pub use delivery::*;
pub use instrument::*;
pub use report::*;

/// Which binary was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Release,
    Debug,
}

/// What a probe is allowed to conclude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The probe found something, and its canary had fired.
    Found(Vec<String>),
    /// Nothing found, AND the canary fired — so the instrument reached the code.
    Clean,
    /// The canary did not fire: the probe proved nothing, either way.
    InstrumentNotValidated(String),
}

/// A silence is only worth something if the instrument first showed it could see.
///
/// This is the rule that was missing when a fuzz of 2,568 variants returned "0 on
/// 1.1.9" on a version where one line crashed for sure: the probe files were
/// named `.dat`. Without a canary, "nothing found" and "nothing reached"
/// return the same word.
pub fn conclude(canary_fired: bool, findings: Vec<String>) -> Verdict {
    if !canary_fired {
        return Verdict::InstrumentNotValidated(
            "the canary did not reproduce the known defect — the probe does not reach \
             the code it claims to measure, so its silence says nothing"
                .to_string(),
        );
    }
    if findings.is_empty() {
        Verdict::Clean
    } else {
        Verdict::Found(findings)
    }
}

/// The result of ONE invocation of the CLI.
#[derive(Debug)]
pub struct Run {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `None` = killed by a signal. An OOM kill is NOT a success.
    pub code: Option<i32>,
    ms: u128,
    profile: Profile,
}

impl Run {
    pub fn bytes(&self) -> usize {
        self.stdout.len()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Did the process terminate on its own?
    pub fn survived(&self) -> bool {
        self.code.is_some()
    }

    /// The measured time, **only if it is comparable**.
    ///
    /// In debug it is not: measured 2.85 s/MB in debug versus 66 ms/MB in
    /// release on the same work, i.e. 43×. A before/after ratio stays
    /// valid in debug; an ABSOLUTE VALUE means nothing there.
    pub fn ms_comparable(&self) -> Option<u128> {
        match self.profile {
            Profile::Release => Some(self.ms),
            Profile::Debug => None,
        }
    }

    /// The raw time, for a before/after RATIO within the same profile.
    pub fn ms_ratio_only(&self) -> u128 {
        self.ms
    }
}

/// A bench: one binary, a fresh state, a fixtures directory.
pub struct Bench {
    exe: PathBuf,
    profile: Profile,
    state: PathBuf,
    work: PathBuf,
    /// `false` = the CLI reads the operator's REAL state. See
    /// [`Bench::reading_real_state`]: it is a named exception, not an
    /// oversight, and it forbids any compaction measurement.
    isolated: bool,
    exe_found: bool,
}

impl Bench {
    /// The only commands a NON-isolated bench is allowed to run: their
    /// answer is the operator's state, not a fold to measure.
    const READS_REAL_STATE: &'static [&'static str] = &["gain", "dashboard", "health"];

    /// Resolves the binary (release preferably) and opens a fresh state.
    ///
    /// `TOKENADE_BENCH_EXE` forces a specific binary — that is what makes it possible to
    /// measure a PUBLISHED version next to the current tree.
    /// A bench pointed at a SPECIFIC binary, without touching the environment.
    ///
    /// `TOKENADE_BENCH_EXE` does the same for a command-line caller, but an
    /// environment variable is global to the process:
    /// a test that sets it sets it for its neighbours. This constructor is the
    /// side-effect-free version, for tests that need a specific binary.
    pub fn with_exe(exe: PathBuf) -> std::io::Result<Bench> {
        let mut b = Self::new()?;
        b.profile = if tokenade_core::platform::path_has_segment(&exe, "release") {
            Profile::Release
        } else {
            Profile::Debug
        };
        b.exe_found = exe.exists();
        b.exe = exe;
        Ok(b)
    }

    pub fn new() -> std::io::Result<Bench> {
        // Resolving the binary does NOT fail construction: on a fresh
        // clone there is nothing in `target/`, and a bench that refuses to be
        // born would bring down the whole workspace suite. The absence surfaces
        // in `run`, where it matters.
        let (exe, profile, found) = Self::resolve_exe();
        let root = std::env::temp_dir().join(format!(
            "tokenade-bench-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let state = root.join("state");
        let work = root.join("work");
        std::fs::create_dir_all(&state)?;
        std::fs::create_dir_all(&work)?;
        Ok(Bench {
            exe,
            profile,
            state,
            work,
            isolated: true,
            exe_found: found,
        })
    }

    /// A bench that lets the CLI read the operator's REAL state.
    ///
    /// It exists only for commands whose answer IS that state:
    /// `gain`, `dashboard`, `health`. Isolating them would return an empty ledger and
    /// a value ranking derived from nothing.
    ///
    /// **Forbidden for a compaction measurement**: without a fresh dedup store,
    /// the second arm of an A/B sees the duplicate the first one just
    /// recorded. [`Bench::run`] refuses it.
    pub fn reading_real_state() -> std::io::Result<Bench> {
        let mut b = Bench::new()?;
        b.isolated = false;
        Ok(b)
    }

    /// Does this bench isolate the state?
    pub fn is_isolated(&self) -> bool {
        self.isolated
    }

    /// The path of the binary, ABSOLUTE.
    ///
    /// The child is launched with `current_dir(work)`, a temporary directory:
    /// a relative path is resolved there against THAT directory and not against the one
    /// we measure from. `TOKENADE_BENCH_EXE=target/debug/tokenade` did
    /// exist for `p.exists()` — which reads from the bench's current
    /// directory — and the launch answered `No such file or directory (os error
    /// 2)`, without saying which of the two paths was missing. It is the same defect
    /// as on the ARGUMENTS, in the same place, on the executable side.
    fn absolutise(p: PathBuf) -> (PathBuf, bool) {
        match std::fs::canonicalize(&p) {
            Ok(abs) => (abs, true),
            Err(_) => {
                let found = p.exists();
                (p, found)
            }
        }
    }

    /// The workspace root. A single spelling: `resolve_exe` looks there for
    /// the binary and [`source_newer_than_exe`](Self::source_newer_than_exe) looks there
    /// for the sources, and both must talk about the same tree.
    pub fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    /// Where cargo puts its builds: `CARGO_TARGET_DIR` when set (relative to
    /// the workspace root, as cargo reads it), `<workspace>/target` otherwise.
    /// Hard-wired as the workspace's `target/` at five sites, the bench
    /// measured and cleaned a directory that a `CARGO_TARGET_DIR` build never
    /// wrote (2026-10-03: two tests red in a clean clone built elsewhere).
    pub fn target_dir() -> PathBuf {
        match std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
            Some(d) => Self::workspace_root().join(d),
            None => Self::workspace_root().join("target"),
        }
    }

    fn resolve_exe() -> (PathBuf, Profile, bool) {
        if let Some(p) = std::env::var_os("TOKENADE_BENCH_EXE") {
            let p = PathBuf::from(p);
            let profile = if tokenade_core::platform::path_has_segment(&p, "release") {
                Profile::Release
            } else {
                Profile::Debug
            };
            let (p, found) = Self::absolutise(p);
            return (p, profile, found);
        }
        let target = Self::target_dir();
        for (dir, profile) in [("release", Profile::Release), ("debug", Profile::Debug)] {
            let cand = target.join(dir).join("tokenade");
            if cand.exists() {
                return (cand, profile, true);
            }
        }
        (
            target.join("release").join("tokenade"),
            Profile::Release,
            false,
        )
    }

    pub fn profile(&self) -> Profile {
        self.profile
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// Where THIS bench's state lives. Fresh, so the dedup has nothing to remember.
    pub fn state_dir(&self) -> &Path {
        &self.state
    }

    /// **The most recent source this binary should have compiled**, if it
    /// is older than it.
    ///
    /// An instrument must never measure a binary older than the tree.
    /// MEASURED 2026-09-08: the same delivery census returned 273,366
    /// then 284,024 bytes on the SAME base, a 4 % gap — enough to sign off a
    /// fold that gains nothing. I first blamed the dedup and wrote a state
    /// reset; it changed **0 captures out of 265**, which pointed at the real
    /// culprit: a `target/debug/tokenade` from a previous iteration.
    ///
    /// Returned rather than judged on the spot: the caller decides
    /// whether to refuse, and the test suite does not have to recompile to run.
    pub fn source_newer_than_exe(&self) -> Option<PathBuf> {
        let built = std::fs::metadata(&self.exe)
            .and_then(|m| m.modified())
            .ok()?;
        // The crates that COMPILE this binary, not all of `crates/`: editing the
        // bench itself does not make the CLI stale, and a guard that refuses wrongly
        // ends up bypassed.
        let root = Self::workspace_root().join("crates");
        ["tokenade-core", "tokenade-cli"].into_iter().find_map(|c| {
            source_newer_than_except(&root.join(c), built, &is_not_compiled_into_the_binary)
        })
    }

    /// **Forgets what a previous measurement left in the state.**
    ///
    /// To call between two measurements meant to be INDEPENDENT — the normal case
    /// of a census, where each capture stands for a session on its
    /// own.
    ///
    /// The dedup answers on CONTENT and not on name, so writing each
    /// capture under a unique name is not enough: `linux/mount_full_en` carries
    /// the same bytes as `linux/mount_full` and came back as a duplicate. And
    /// it has a WINDOW, so it bites or not depending on the speed of the
    /// pass: MEASURED 2026-09-08, the same capture returned 8,995 bytes then
    /// 344 on the same tree, and a first try of this guard concluded "0
    /// captures changed" on a pair of passes where it had bitten neither
    /// one nor the other. An intermittent non-reproducibility is measured over
    /// several passes, never over two.
    pub fn forget(&self) {
        for st in tokenade_core::stores::ISOLATABLE {
            let _ = std::fs::remove_file(self.state.join(st.file));
        }
        let _ = std::fs::remove_dir_all(self.state.join("stash"));
    }

    /// Writes a fixture **with its real extension** and returns its path.
    ///
    /// The extension is not decorative: it is what routing uses to pick
    /// a compactor. `stem` without a dot, `ext` without a dot.
    pub fn fixture(&self, stem: &str, ext: &str, body: &str) -> std::io::Result<PathBuf> {
        assert!(
            !ext.starts_with('.'),
            "ext without a dot: `html`, not `.html` (got {ext:?})"
        );
        assert!(
            !ext.is_empty(),
            "a fixture WITHOUT an extension reaches no compactor — that is the \
             trap that made a fuzz return \"0 hits\" on a faulty version"
        );
        let p = self.work.join(format!("{stem}.{ext}"));
        std::fs::write(&p, body)?;
        Ok(p)
    }

    /// Runs the CLI and captures its bytes. The output never touches a
    /// terminal, so no hook compacts it on the way.
    pub fn run(&self, args: &[&str]) -> std::io::Result<Run> {
        self.run_with_stdin(args, None)
    }

    /// The same thing, pushing `input` onto the child's standard input.
    ///
    /// That is what makes it possible to measure a capture UNDER ITS PROGRAM without
    /// fabricating a fake executable: `tokenade filter <prog>` compacts stdin
    /// "as if" that program had produced it. A previous version put
    /// an executable shim at the head of the child's `PATH` — a choice already ruled out
    /// in this repo, and for good reason: the shim's body said `cat`, the corpus
    /// contains a capture named `cat`, and the shim relaunched itself.
    /// Fork bomb, machine memory exhausted, two hard shutdowns.
    pub fn run_with_stdin(&self, args: &[&str], input: Option<&str>) -> std::io::Result<Run> {
        self.run_in(None, args, input)
    }

    /// **Run from a REAL directory**, the state staying isolated.
    ///
    /// `map`, `query`, `impact`, `skeleton` and `semantic` answer from an
    /// index built on the code of the current directory. The bench launched everything
    /// from its empty bin, so these five commands — five rows of the value
    /// ranking — answered "0 files indexed" and were measurable by
    /// NO instrument in the repo.
    ///
    /// The state (`TOKENADE_CONFIG_DIR`, reserve, dedup) stays the bench's: what
    /// is real here is the CODE read, not the store written. A real `cwd`
    /// nonetheless lets a command write in the caller's tree — give it only
    /// to commands that READ.
    pub fn run_in(
        &self,
        cwd: Option<&Path>,
        args: &[&str],
        input: Option<&str>,
    ) -> std::io::Result<Run> {
        assert!(
            self.isolated || Self::READS_REAL_STATE.contains(&args.first().copied().unwrap_or("")),
            "`reading_real_state` is for commands whose answer IS that state \
             ({:?}), not for measuring a compaction without a fresh dedup store",
            Self::READS_REAL_STATE
        );
        if !self.exe_found {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "no tokenade binary to measure ({}) — `cargo build` first, \
                     or point TOKENADE_BENCH_EXE",
                    self.exe.display()
                ),
            ));
        }
        // The current directory is the bench's, so that a file the
        // command might write stays in the bin. Consequence: a RELATIVE
        // argument from the caller no longer resolves. Measured while writing `probe`:
        // `read crates/.../grep_matches.txt` returned "delivered 0 bytes, −100 %"
        // — a refusal read as a perfect compaction. An argument that names
        // an existing file is therefore made absolute here, once, rather than
        // worked around by each caller.
        let resolved: Vec<String> = args
            .iter()
            .map(|a| match std::fs::canonicalize(a) {
                Ok(p) => p.to_string_lossy().into_owned(),
                Err(_) => (*a).to_string(),
            })
            .collect();
        let mut c = Command::new(&self.exe);
        c.args(&resolved).current_dir(cwd.unwrap_or(&self.work));
        if self.isolated {
            c.env("TOKENADE_CONFIG_DIR", &self.state)
                .env("TOKENADE_STASH_DIR", self.state.join("stash"));
            // ALL the stores, from the product's inventory. Three were
            // redirected by hand and two forgotten: the index, the fold
            // store, the file search and the semantic one stayed the
            // operator's, so an "isolated" bench READ them — and
            // `probe --here index` rebuilt the machine's REAL index.
            for st in tokenade_core::stores::ISOLATABLE {
                c.env(st.env, self.state.join(st.file));
            }
        }
        // **A measurement does not depend on the environment of whoever runs it.**
        //
        // MEASURED 2026-09-09: the same capture returns 7,825 bytes in a
        // clean environment and 3,085 when `TOKENADE_PRESSURE=0.35` lingers
        // in the shell — the session wrapper exports it, and every manual run
        // inherits it. Two figures a factor of 2.5 apart were opposed
        // for half an hour with nothing saying why.
        //
        // The inventory lives in the PRODUCT, next to the stores: that one says
        // where the command writes, this one what it decides.
        for var in tokenade_core::stores::COMPACTION_ENV {
            c.env_remove(var);
        }
        // The crate is not swept by the spawn guards of `cli`/`core`,
        // but a bench that opens a console on Windows is still a broken bench.
        tokenade_core::process_util::configure_no_window(&mut c);
        // "Who kills this if the bench is SIGKILLed?" — the repo's guard asks
        // the question and it is right: a measurement may launch a long command,
        // and `output()` returns no handle on the child. So we adopt it
        // explicitly, like `wrap`: the group only EXISTS if a terminal does not
        // already own it, and the reaper holds the guarantee for the kill
        // we never see happen.
        c.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(match input {
                Some(_) => std::process::Stdio::piped(),
                None => std::process::Stdio::null(),
            });
        let t = Instant::now();
        let mut child = c.spawn()?;
        // Push the input BEFORE waiting: `wait_with_output` blocks, and a
        // child that reads stdin would never give control back.
        if let Some(text) = input {
            use std::io::Write as _;
            if let Some(mut sink) = child.stdin.take() {
                sink.write_all(text.as_bytes())?;
            }
        }
        let reaper = if tokenade_core::process_util::terminal_owns_the_group() {
            None
        } else {
            tokenade_core::process_util::guard_group_against_our_death(child.id())
        };
        let out = child.wait_with_output()?;
        if let Some(r) = reaper {
            r.stand_down();
        }
        Ok(Run {
            stdout: out.stdout,
            stderr: out.stderr,
            code: out.status.code(),
            ms: t.elapsed().as_millis(),
            profile: self.profile,
        })
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        // A bench that leaves its state behind makes the next one lie.
        if let Some(root) = self.state.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

#[cfg(test)]
include!("lib_tests.rs");
