//! **When the measurement is worth NOTHING.**
//!
//! The 37 recorded instrument faults all have the same shape: the bench
//! returned a figure, and the figure did not measure what we thought. A binary
//! older than the tree, a probe file the routing does not reach, a
//! passthrough that was never activated, a fold gate that refuses before
//! working, a build cache that ext4 can no longer index.
//!
//! Everything that answers "this reading is void, and here is why" lives here. Nothing
//! here MEASURES: each function refuses, or names the reason for the refusal.

use super::*;

// **Is this bench binary older than the bench?**
//
// The stale-binary guard refuses an outdated `tokenade` — and let an outdated
// INSTRUMENT through. MEASURED 2026-09-08: `cargo build -p tokenade-bench --bin
// probe` writes `target/debug/probe`, I was running `target/release/probe`, and
// for three rounds the instrument executed dated from ten hours earlier. A
// flag I had just added did not exist in the launched binary, and its
// refusal talked about something else.
//
// The same question, for the other binary: that is what making the class
// impossible rather than fixing the instance means.

pub fn refuse_if_this_instrument_is_stale() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // Under `cargo test`, the executable is a test binary recompiled on
    // demand: the question makes no sense and its answer would always be yes.
    if tokenade_core::platform::path_has_segment(&exe, "deps") {
        return;
    }
    let Ok(built) = std::fs::metadata(&exe).and_then(|m| m.modified()) else {
        return;
    };
    let src = Bench::workspace_root().join("crates/tokenade-bench/src");
    // ANOTHER instrument's binary does not go into this one: cargo does not
    // relink it, so blaming it makes the refusal INESCAPABLE — measured while
    // using it, `mutate` refused to run because of an edit to
    // `value_ranking.rs`, and no rebuild could lift the
    // verdict. A guard that cannot be satisfied ends up bypassed.
    let mine = exe.file_stem().map(|s| s.to_string_lossy().into_owned());
    let bins = src.join("bin");
    // `src/main.rs` and `src/benches/**` belong to the PACKAGE binary
    // (`tokenade-bench`), and to it alone: no instrument of `src/bin/` links
    // them. Counting them amounted to refusing to run for an edit that
    // could not change what the instrument measures — and the refusal was
    // inescapable, exactly like the one for the `x_tests.rs` siblings: `cargo build
    // --bins` finished in 0.15 s without rebuilding anything.
    let own_main = src.join("main.rs");
    let own_families = src.join("benches");
    let belongs_to_the_package_binary =
        |p: &Path| p == own_main.as_path() || p.starts_with(&own_families);
    let i_am_the_package_binary = mine.as_deref() == Some("tokenade-bench");
    let another_instrument = |p: &Path| {
        is_not_compiled_into_the_binary(p)
            || instrument_owning(&bins, p).is_some_and(|owner| Some(owner) != mine)
            || (!i_am_the_package_binary && belongs_to_the_package_binary(p))
    };
    if let Some(newer) = source_newer_than_except(&src, built, &another_instrument) {
        eprintln!("{}", stale_exe_note(&exe, &newer));
        eprintln!(
            "\x20          (it is the INSTRUMENT that is stale, not the product:\n\
             \x20           `cargo build --release -p tokenade-bench --bin <name>`)"
        );
        std::process::exit(2);
    }
}

/// **Which instrument does a file of `src/bin/` serve?**
///
/// `src/bin/mutate.rs` serves `mutate`. `src/bin/value_ranking/sessions.rs` serves
/// `value_ranking`: a binary's root mounts its children from a directory
/// named after it, and they are linked only into that one.
///
/// The question used to be asked on the file's PARENT only, which only saw
/// the first case. The day an instrument took children, editing
/// `value_ranking/sessions.rs` made `duplication` "stale" — and the refusal
/// was INESCAPABLE: cargo does not relink `duplication` for that edit,
/// so no rebuild could make its binary newer.
pub(crate) fn instrument_owning(bins: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(bins).ok()?;
    let first = rel
        .components()
        .next()?
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    Some(
        first
            .strip_suffix(".rs")
            .map(str::to_string)
            .unwrap_or(first),
    )
}

/// **How many simultaneous links does the AVAILABLE memory tolerate?**
///
/// A link in this repo costs ~2.4 GB at peak (`lto = "fat"`, `codegen-units = 1`
/// : each binary links the whole graph). The cap in `.cargo/config.toml`
/// is static and assumes a free machine; it is not always one.
/// MEASURED 2026-09-09: with two foreign `python3` at 2.3 and 1.8 GB and
/// free memory dropped below 8 GB, **ten runs in a row were killed**
/// "because the system is low on memory" — and the same mutation passed
/// on the first try with `CARGO_BUILD_JOBS=2`.
///
/// It was not a byte cap that was missing, it was the number of links. It
/// can be computed: one link per 3 GB actually available, between 1
/// and 8. The caller keeps the last word by setting the variable itself.
pub fn safe_build_jobs() -> usize {
    const GB_PER_LINK: u64 = 3 * 1024 * 1024; // in KiB, like /proc/meminfo
    let available = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("MemAvailable:"))
                .and_then(|v| v.split_whitespace().next().map(str::to_string))
        })
        .and_then(|v| v.parse::<u64>().ok())
        // Without /proc, stay cautious rather than open wide.
        .unwrap_or(GB_PER_LINK * 2);
    (available / GB_PER_LINK).clamp(1, 8) as usize
}

/// **A directory that the ext4 index can no longer extend returns `ENOSPC` with the
/// disk three-quarters empty.**
///
/// MEASURED 2026-09-08: `target/debug/deps` held **4,744,341 entries**, and
/// `cargo` answered `No space left on device` while linking, on a system
/// showing **414 GB free**. Two symptoms followed, and neither
/// named the cause: `TREE BROKEN AFTER RESTORE` on an intact tree, and
/// six runs killed by the supervisor "for low memory" while
/// the kernel showed `/proc/pressure/memory avg10=0.00`.
///
/// What filled it is known: each distinct feature set produces a distinct set
/// of artifacts, and the mutation harness rebuilds for every mutant.
///
/// The build cache is REGENERABLE: beyond the cap we remove it, we
/// say so, and `cargo` rebuilds it. It is an instruction that can be mechanised, so
/// it does not remain an instruction.
pub(crate) const BUILD_CACHE_MAX_ENTRIES: usize = 100_000;
/// Does the directory hold more entries than the cap? Counting is BOUNDED:
/// on a directory of four million names, the question must be answered
/// without reading them all.
pub fn build_cache_is_unindexable(deps: &std::path::Path, max: usize) -> bool {
    match std::fs::read_dir(deps) {
        Ok(rd) => rd.take(max + 1).count() > max,
        Err(_) => false,
    }
}

pub fn prune_a_build_cache_that_ext4_can_no_longer_index() {
    // Under `cargo test`, the executable IS in `deps/`: purging it would amount
    // to removing the directory the deciding process runs from, and the
    // other binaries of the same pass with it. Same exclusion as the
    // stale-instrument refusal, and for the same reason.
    if std::env::current_exe()
        .map(|e| tokenade_core::platform::path_has_segment(&e, "deps"))
        .unwrap_or(true)
    {
        return;
    }
    let target = Bench::target_dir();
    for sub in ["debug", "release"] {
        let deps = target.join(sub).join("deps");
        if !build_cache_is_unindexable(&deps, BUILD_CACHE_MAX_ENTRIES) {
            continue;
        }
        let incr = target.join(sub).join("incremental");
        let _ = std::fs::remove_dir_all(&deps);
        let _ = std::fs::remove_dir_all(&incr);
        eprintln!(
            "  ℹ build cache purged: target/{sub}/deps held more than \
             {BUILD_CACHE_MAX_ENTRIES} entries, which makes ext4 return ENOSPC well \
             before the disk is full. `cargo` will rebuild it."
        );
    }
}

/// Refuse an argument this bench does not understand, rather than silently
/// measuring something else.
///
/// FOUND while using it: `duplication -- 4` — the form without the
/// flag name — was ignored, the tool measured at the DEFAULT threshold and I read its
/// answer as the one to my question. Only the printed header, which recalls the
/// threshold, kept me from concluding on a figure that answered something else.
/// `testsel --min-lines` is worse: it takes the mistyped flag for a
/// FILE name and answers "no testable module", which looks like a result.
///
/// It is the rule the product already holds for itself
/// (`gain --history --json` silently dropped `--history` and now refuses
/// it). It applies first to the INSTRUMENT: a wrong measurement steers
/// a whole iteration.
pub fn reject_unknown_args(known: &[&str], args: &[String]) {
    // Placed HERE because it is the seam every instrument goes through: a
    // guard one has to remember to call is called only by those who
    // remembered.
    refuse_if_this_instrument_is_stale();
    prune_a_build_cache_that_ext4_can_no_longer_index();
    let mut expecting_value = false;
    for a in args {
        if expecting_value {
            expecting_value = false;
            continue;
        }
        if !a.starts_with('-') {
            continue;
        }
        let name = a.split('=').next().unwrap_or(a);
        if known.contains(&name) {
            // A flag with a value consumes the next word, unless it has
            // already glued it with `=`.
            expecting_value = !a.contains('=');
            continue;
        }
        eprintln!(
            "unknown argument: {a}\nthis bench knows: {}\n\nAn instrument that ignores an \
             argument measures something other than what it is asked, and says so with the \
             confidence of an answer.",
            if known.is_empty() {
                "no flag".to_string()
            } else {
                known.join(", ")
            }
        );
        std::process::exit(2);
    }
}

/// Did this bench measure NOTHING?
///
/// Extracted to be testable: the decision lived inline in the `main` of
/// `compactor_review`, so nothing could exercise it and it was LOST while
/// parallelising this bench — the count survived in each slice and nobody
/// read it any more. Only a `warning: assigned but never used` said so.
/// The extraction is what makes the error expressible.
///
/// One fold in a hundred is enough: the threshold does not judge the quality of the compaction,
/// it tells "nothing to fold" from "I did not look" — broken
/// routing, empty corpus.
pub fn instrument_is_dead(folded: usize, calls: usize) -> bool {
    calls > 0 && folded * 100 < calls
}

/// **Does this file GO INTO a binary?**
///
/// A crate's `tests/`, `benches/` and `examples/` are not compiled into the
/// product: editing them makes no binary stale. The staleness guard
/// swept `crates/tokenade-core` entirely, so editing
/// `tests/support/corpus.rs` made a measurement refuse and cost a
/// full rebuild of the CLI — paid for once, and one iteration in
/// two touches a test file.
///
/// The rule holds for BOTH guards: the product's
/// ([`Bench::source_newer_than_exe`]) and the instruments'
/// ([`refuse_if_this_instrument_is_stale`]), which composes it with its own
/// filter. *A guard that refuses wrongly ends up bypassed* — the comment already
/// said so, one granularity up.
pub fn is_not_compiled_into_the_binary(p: &Path) -> bool {
    // An `x_tests.rs` sibling is mounted by `include!` under `#[cfg(test)]`: in
    // a binary build, the inclusion is not even expanded, so
    // cargo does not count it as a dependency and DOES NOT RELINK the instrument
    // again. Counting it here made the refusal inescapable — measured by editing
    // `lib_tests.rs`: `cargo build --bins` finished in 0.14 s without rebuilding
    // anything, and the instrument still refused to run. That is
    // exactly the shape the comment above says not to repeat.
    if p.file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with("_tests.rs"))
    {
        return true;
    }
    p.components().any(|c| {
        matches!(
            c.as_os_str().to_string_lossy().as_ref(),
            "tests" | "benches" | "examples"
        )
    })
}

pub fn source_newer_than(root: &Path, built: std::time::SystemTime) -> Option<PathBuf> {
    source_newer_than_except(root, built, &|_| false)
}

/// Like [`source_newer_than`], skipping the files `skip` designates.
///
/// Exists because "newer than the binary" is the right question only
/// for the sources that GO INTO that binary. Without the escape, the
/// stale-instrument refusal became inescapable as soon as another instrument of the same crate was
/// touched.
pub fn source_newer_than_except(
    root: &Path,
    built: std::time::SystemTime,
    skip: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rs")
                && !skip(&p)
                && std::fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .is_ok_and(|m| m > built)
            {
                return Some(p);
            }
        }
    }
    None
}

/// What an instrument that refuses to measure a stale binary must say.
pub fn stale_exe_note(exe: &Path, src: &Path) -> String {
    format!(
        "WARNING  {} is OLDER than {}: this measurement would bear on\n\
         \x20          a version that is no longer the tree. Rebuild first:\n\
         \x20            cargo build -p tokenade-cli",
        exe.display(),
        src.display()
    )
}
