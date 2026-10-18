//! One sweep for every temporary directory this project's tests leave behind.
//!
//! ## Why this file exists
//!
//! 236 places across the two crates build a scratch directory as
//! `temp_dir().join(format!("tokenade-<tag>-{pid}"))`. **Not one of them
//! removes it afterwards.** Each helper clears its OWN path on the way in, but
//! the name carries the pid, so the next `cargo test` gets a fresh pid, a
//! fresh directory, and the previous run's is orphaned for good.
//!
//! Measured on this machine 2026-08-27, real blocks and not apparent size:
//! **7.05 GB across 83 633 entries**, every one of them ours —
//! `tokenade-claudecode-estimate-cap` 5 182 MB over 259 copies,
//! `tokenade-offer` 550 MB over 308, `tokenade-e2e-bin` 597 MB,
//! `tokenade-e2e-help-reach` 11 856 separate entries. On Arch and most
//! systemd distributions `/tmp` is a tmpfs, so that is RAM.
//!
//! It is not a cosmetic leak: it broke this project's own e2e suite three
//! times in one session, with `No space left on device` on a binary copy the
//! first time and a silent, rising failure count (2, then 5, then 8) the other
//! two — which reads exactly like a regression in whatever is being changed.
//!
//! ## Why here, and why a sweep rather than a Drop guard
//!
//! `tests/e2e/main.rs` has done the right thing since it was written, for one
//! prefix, and says why in its own comment: siblings "whose process is gone
//! are swept on the way in, because nothing else would ever remove them". One
//! rule, one site, never generalised — the shape this project keeps finding.
//!
//! A `Drop` guard would be tidier and would fix only the sites that adopt it;
//! 236 call sites is a large mechanical change to land at once, and a panicking
//! test skips `Drop` anyway. A sweep is different: it removes any stale
//! `tokenade-*` sibling, not just its own prefix, so **one** adopting site
//! cleans up after all the others, including the ones nobody ever converts.
//!
//! A directory is stale when the pid embedded in its name is not a live
//! process. A pid that has been recycled by something unrelated reads as live
//! and the directory is kept — the conservative direction, and the only one
//! worth taking when the alternative is deleting a running test's scratch.

use std::path::Path;

/// Every prefix this project's tests put in front of `{pid}` in the temp dir.
///
/// The module's own promise is that "one adopting site cleans up after all the
/// others, including the ones nobody ever converts" — and that promise is only
/// as wide as this list. It held exactly one entry, `tokenade-`, and the
/// biggest leak on this machine did not carry it: `tkn-sess-refs-{pid}`, from
/// the session-attribution test in `disclose.rs`, which sets `HOME` to the
/// directory and stashes into it. Measured 2026-09-02, real blocks:
/// **46 orphans, 6.0 GB, on an 8 GB tmpfs** — dated back to 08-29, and the
/// reason the e2e suite failed that day with `No space left on device` on a
/// binary copy. The sweep written to stop precisely that walked past it,
/// because a directory named for this project did not spell the project's
/// name.
///
/// Add a prefix here rather than renaming a directory: renaming fixes the next
/// run, this fixes the ones already on disk.
///
/// Two entries were still not the list. Measured 2026-10-02 with `/tmp` at
/// 100 % — the e2e `gain` suite failing up to seven tests a pass on
/// `StorageFull`, and read for days as "flaky under load": `tok-badge-*`
/// (998 MB over two orphans) and `tok-cline-*` (417 MB), from this project's
/// own install tests. A census of the names the code builds found 46 `tok-`,
/// 27 `tk-`, 8 `tkd-`, 5 `tkn_` and 3 `tk_` sites; the guard
/// `every_scratch_name_the_code_builds_is_swept` now derives them from the
/// sources instead of from memory.
const SCRATCH_PREFIXES: &[&str] = &["tokenade-", "tkn-", "tkn_", "tkd-", "tok-", "tk-", "tk_"];

/// Remove `tokenade-*` scratch directories in the temp dir whose owning
/// process is gone. Returns how many were removed.
///
/// Cheap enough to call from a test helper: one `read_dir` of the temp
/// directory and a `stat` per candidate.
pub fn sweep_stale_scratch_dirs() -> usize {
    sweep_in(&std::env::temp_dir())
}

/// The sweep, against a root the caller names.
///
/// Split out for the tests, and that split is not cosmetic. They used to run
/// against the real temp directory, and a mutation of the `tokenade-` prefix
/// check — the exact mutation that proves the check matters — then deleted
/// unrelated files belonging to the process running the tests, this session's
/// own scratch directory among them. A destructive function must be
/// exercisable somewhere it cannot do that.
pub fn sweep_in(root: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0usize;
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = SCRATCH_PREFIXES.iter().find_map(|p| name.strip_prefix(p)) else {
            continue;
        };
        let pids = pid_segments(rest);
        if pids.is_empty() || pids.into_iter().any(process_is_alive) {
            continue;
        }
        let path = e.path();
        let ok = if path.is_dir() {
            std::fs::remove_dir_all(&path).is_ok()
        } else {
            std::fs::remove_file(&path).is_ok()
        };
        if ok {
            removed += 1;
        }
    }
    removed
}

/// Every all-digit `-`- or `_`-separated segment (`tkn_gate_p0_{pid}` spells
/// it with underscores). The owner's pid is one of them, but not always the
/// first: `tmp_home("trt-738")` builds `tokenade-claudecode-trt-738-{pid}`,
/// and reading `738` as the owner let any concurrent sweep delete a RUNNING
/// test's home whenever pid 738 was dead. The directory is stale only when
/// none of them is alive — wrong in the direction of keeping a directory,
/// never of deleting a live one.
fn pid_segments(rest: &str) -> Vec<u32> {
    rest.split(['-', '_'])
        .filter(|seg| !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|seg| seg.parse().ok())
        .collect()
}

/// Delegates to [`crate::process_util::process_is_alive`] — the liveness
/// question has ONE declarant. This module carried its own copy, and the copy
/// asked `/proc`, which is a Linux question `cfg(unix)` also asks on macOS.
fn process_is_alive(pid: u32) -> bool {
    crate::process_util::process_is_alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_reaches_every_prefix_the_tests_actually_use() {
        // Named, not derived: the leak that filled an 8 GB tmpfs was a
        // directory whose prefix this list did not carry, so a test that asks
        // the list about itself would have passed throughout.
        let root = std::env::temp_dir().join(format!(
            "tokenade-sweep-prefix-guard-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        // A pid that is certainly not alive.
        let dead = "999999999";
        for name in [
            format!("tokenade-offer-{dead}"),
            format!("tkn-sess-refs-{dead}"),
        ] {
            let d = root.join(&name);
            std::fs::create_dir_all(d.join("stash")).expect("scratch");
            std::fs::write(d.join("stash").join("x.txt"), "payload").expect("write");
        }
        // A live-pid sibling and a foreign directory must both survive.
        let mine = root.join(format!("tkn-sess-refs-{}", std::process::id()));
        std::fs::create_dir_all(&mine).expect("mine");
        let foreign = root.join(format!("someone-elses-{dead}"));
        std::fs::create_dir_all(&foreign).expect("foreign");

        let removed = sweep_in(&root);
        assert_eq!(removed, 2, "both stale prefixes must be swept");
        assert!(!root.join(format!("tokenade-offer-{dead}")).exists());
        assert!(
            !root.join(format!("tkn-sess-refs-{dead}")).exists(),
            "the tkn- prefix is the 6 GB one; it must be swept"
        );
        assert!(mine.exists(), "a live pid must be kept");
        assert!(
            foreign.exists(),
            "a directory that is not ours must be kept"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn every_numeric_segment_is_a_pid_candidate() {
        assert_eq!(pid_segments("offer-991-7"), vec![991, 7]);
        assert_eq!(pid_segments("fp-large-991-ThreadId(3)"), vec![991]);
        assert_eq!(
            pid_segments("claudecode-trt-738-1362985"),
            vec![738, 1362985]
        );
        assert!(pid_segments("no-pid-here").is_empty());
    }

    /// A label with digits BEFORE the pid: the live owner keeps its directory
    /// even when the label's number names a dead process.
    #[test]
    fn a_numeric_label_before_the_pid_does_not_condemn_a_live_dir() {
        let root = isolated_root("label");
        let mine = root.join(format!(
            "tokenade-claudecode-trt-4000000-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&mine).unwrap();
        sweep_in(&root);
        assert!(mine.exists(), "a running test's home was swept");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every scratch name the code builds (a `format!` literal joined onto
    /// `temp_dir()`) must
    /// start with a swept prefix. Derived from the SOURCES, not from the list:
    /// the list was written from the leaks someone had noticed, twice short.
    #[test]
    fn every_scratch_name_the_code_builds_is_swept() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/");
        let mut stack = vec![crates.to_path_buf()];
        let (mut seen, mut stray) = (0usize, Vec::new());
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read_dir").flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n != "target") {
                        stack.push(p);
                    }
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                for name in scratch_names(&src) {
                    seen += 1;
                    if !SCRATCH_PREFIXES.iter().any(|pre| name.starts_with(pre)) {
                        stray.push(format!("{}: {name}", p.display()));
                    }
                }
            }
        }
        assert!(
            seen > 300,
            "the scan reached only {seen} names: it no longer reads the sources"
        );
        assert!(
            stray.is_empty(),
            "scratch names no sweep reaches:\n{}",
            stray.join("\n")
        );
    }

    /// The literal opening each `format!` joined onto `temp_dir()` — whitespace and
    /// line breaks allowed between the calls, as rustfmt writes them.
    fn scratch_names(src: &str) -> Vec<&str> {
        let mut out = Vec::new();
        for (i, _) in src.match_indices("temp_dir()") {
            let rest = src[i + "temp_dir()".len()..].trim_start();
            let Some(rest) = rest.strip_prefix(".join(") else {
                continue;
            };
            let Some(rest) = rest.trim_start().strip_prefix("format!(") else {
                continue;
            };
            let Some(rest) = rest.trim_start().strip_prefix('"') else {
                continue;
            };
            out.push(&rest[..rest.find(['"', '{']).unwrap_or(rest.len())]);
        }
        out
    }

    #[test]
    fn the_pid_may_follow_an_underscore() {
        assert_eq!(pid_segments("gate_p0_991"), vec![991]);
    }

    /// A root of this test's own, so a mutation of the sweep cannot reach
    /// anything that matters. See `sweep_in`.
    fn isolated_root(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-sweeproot-{}-{tag}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_live_process_is_never_swept() {
        let root = isolated_root("live");
        let me = std::process::id();
        let d = root.join(format!("tokenade-sweeptest-{me}"));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(sweep_in(&root), 0, "something was swept that should not be");
        assert!(d.exists(), "the sweep deleted a LIVE process's scratch");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dead_process_is_swept_and_a_stranger_is_not() {
        let root = isolated_root("dead");
        // A pid that cannot be running: pid 0 is never a userland process.
        let dead = root.join("tokenade-sweeptest-0");
        std::fs::create_dir_all(dead.join("nested")).unwrap();
        std::fs::write(dead.join("nested").join("f"), b"x").unwrap();
        // Something that is not ours must survive whatever its name looks like.
        let stranger = root.join("someoneelse-0");
        std::fs::create_dir_all(&stranger).unwrap();
        // …and a `tokenade-*` name with no pid in it at all is not ours to
        // judge either.
        let unnumbered = root.join("tokenade-no-pid-here");
        std::fs::create_dir_all(&unnumbered).unwrap();

        assert_eq!(sweep_in(&root), 1, "exactly one entry qualifies");

        assert!(!dead.exists(), "a dead process's scratch survived");
        assert!(stranger.exists(), "the sweep reached outside `tokenade-*`");
        assert!(unnumbered.exists(), "a name with no pid was swept anyway");
        let _ = std::fs::remove_dir_all(&root);
    }
}
