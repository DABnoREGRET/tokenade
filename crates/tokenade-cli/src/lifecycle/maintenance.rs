//! Commands that clean up what tokenade wrote: the stash, the cache, the
//! session transcripts, a leaked secret.
//!
//! All destructive to some degree, so all of them share the same shape: say
//! what would go, then go only when asked.

use super::*;

mod redact;
mod scrub;
pub(crate) use redact::*;
pub(crate) use scrub::*;

/// `tokenade evict-stale` — sweep every embedding/cache DB and drop
/// rows untouched for ≥7 days. Run-once-then-exit; the same logic
/// is invoked opportunistically by some hot paths (e.g. file_search
/// on cache miss), but a manual call lets the user reclaim disk
/// when they know they've been inactive.
/// `tokenade compact-sessions` — mask oversized stale tool_result payloads in
/// idle Claude Code transcripts so a future `/resume` rebuilds cheaper. Sweeps
/// every `~/.claude/projects/*/*.jsonl` except files modified in the last
/// TRIM_FRESHNESS window (which protects any live/recent session). Originals
/// stay recoverable via the stash. The auto trigger at SessionStart does the
/// same for the current project; this is the explicit whole-machine pass.
pub(crate) fn cmd_compact_sessions(rest: &[String]) -> i32 {
    if let Err(code) = validate_simple_bool_flags(rest, "compact-sessions", &["--help", "-h"]) {
        return code;
    }
    let Some(home) = tokenade_core::platform::home_dir() else {
        eprintln!("compact-sessions: no home dir");
        return 1;
    };
    let projects = home.join(".claude").join("projects");
    let Ok(rd) = std::fs::read_dir(&projects) else {
        println!("[tokenade compact-sessions] no Claude Code transcripts found");
        return 0;
    };
    let mut total = tokenade_core::session_compact::CompactStats::default();
    let mut files = 0usize;
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        // No single "active" file in a manual run — freshness guards live ones.
        let st = tokenade_core::session_compact::compact_stale_transcripts(
            &dir,
            std::path::Path::new(""),
            tokenade_core::session_compact::TRIM_FRESHNESS_SECS,
        )
        .unwrap_or_default();
        if st.blocks_masked > 0 {
            files += 1;
        }
        total.blocks_masked += st.blocks_masked;
        total.tokens_before += st.tokens_before;
        total.tokens_after += st.tokens_after;
    }
    println!(
        "[tokenade compact-sessions] masked {} oversized block(s) across {} transcript(s); \
         ~{} tokens won't be re-sent on resume (recoverable via `tokenade expand-ref`)",
        total.blocks_masked,
        files,
        total.saved()
    );
    0
}

/// Report one eviction pass and add what it dropped to the running total.
///
/// Six passes spelled this three-arm match out by hand — `file_search`,
/// `fold_store`, `semantic_db`, `index_db`, the disclose stash and the gain
/// ledger. The MESSAGES differ (a chunk is not a fold is not a stashed file),
/// so they come in as arguments; the SHAPE does not, and it is the shape that
/// kept being copied.
///
/// The two `prune_missing` passes are deliberately NOT folded in: an empty
/// prune must stay silent (no "nothing to evict" line for a secondary pass),
/// and their error names the pass rather than the store.
fn report_evicted<E: std::fmt::Display>(
    label: &str,
    dropped: &str,
    nothing: &str,
    r: Result<usize, E>,
    total: &mut usize,
) {
    match r {
        Ok(n) if n > 0 => {
            println!("  {label}: {n} {dropped}");
            *total += n;
        }
        Ok(_) => println!("  {label}: {nothing}"),
        Err(e) => eprintln!("  {label}: {e}"),
    }
}

/// One store's contribution to the sweep: rows dropped, and bytes actually
/// returned to the filesystem.
///
/// `bytes` is accumulated HERE, not at the VACUUM loop below, because two
/// sweeps free real bytes that the VACUUM loop can never see and so were
/// reported as zero: `dedup_ref::evict_to_size_cap` VACUUMs its own connection
/// (the outer loop then finds nothing left to reclaim), and the disclose stash
/// is plain files with no database to vacuum at all. Measured 2026-08-04 on a
/// dogfood install: dedup.db went 163 → 153 MiB and the command still printed
/// "19.9 MB reclaimed" — the 18.5 MB from index.db plus 1.5 MB from
/// semantic.db, with dedup's ~10 MiB missing.
///
/// Deliberately NOT added: `index_db::evict_to_size_cap`'s figure. It returns
/// the LOGICAL byte_count of the rows it deleted, and those same bytes then
/// show up as the file shrinking in the VACUUM loop. Counting both would
/// over-report — the opposite error, and the harder one to spot.
#[derive(Default)]
struct SweepTally {
    rows: usize,
    bytes: u64,
}

/// A `prune_missing` result, reported the one way both stores report it: a
/// line when rows went, silence when none did, stderr on failure.
fn report_pruned<E: std::fmt::Display>(label: &str, r: Result<usize, E>, total: &mut usize) {
    match r {
        Ok(n) if n > 0 => {
            println!("  {label}: {n} missing-path row(s) pruned");
            *total += n;
        }
        Ok(_) => {}
        Err(e) => eprintln!("  {label} prune-missing: {e}"),
    }
}

fn sweep_dedup_ref(t: &mut SweepTally) {
    match tokenade_core::dedup_ref::evict_stale(&std::path::PathBuf::from("/ignored")) {
        Ok(n) if n > 0 => {
            println!("  dedup_ref: {n} content rows dropped");
            t.rows += n;
        }
        Ok(_) => println!("  dedup_ref: nothing to evict"),
        Err(e) => eprintln!("  dedup_ref: {e}"),
    }
    // OPP-1 — LRU byte-cap eviction (oldest last_used first) to bound
    // dedup.db when a burst of unique outputs blows the budget before
    // the 7-day TTL kicks in. Cap defaults to 150 MiB, env-overridable
    // via TOKENADE_DEDUP_MAX_BYTES.
    {
        let (n, freed) = tokenade_core::dedup_ref::evict_to_size_cap();
        if n > 0 {
            println!(
                "  dedup_ref: {n} row(s) over size cap dropped ({} freed)",
                human_bytes(freed)
            );
            t.rows += n;
            // Safe to add: this path VACUUMs its own connection, so the loop
            // below sees an already-compact file and reports nothing for it.
            t.bytes += freed;
        } else {
            println!("  dedup_ref: under size cap");
        }
    }
}

fn sweep_chunk_stores(t: &mut SweepTally) {
    report_evicted(
        "file_search",
        "chunks dropped",
        "nothing to evict",
        tokenade_core::file_search::evict_stale(),
        &mut t.rows,
    );
    report_evicted(
        "fold_store",
        "folds dropped",
        "nothing to evict",
        tokenade_core::fold_store::evict_stale(),
        &mut t.rows,
    );
}

fn sweep_semantic_db(t: &mut SweepTally) {
    match tokenade_core::semantic_db::open() {
        Ok(conn) => {
            report_evicted(
                "semantic_db",
                "cold project indexes dropped",
                "nothing to evict",
                tokenade_core::semantic_db::evict_stale(&conn),
                &mut t.rows,
            );
            // Also drop rows whose project_path no longer exists on
            // disk (the existing TTL-based evict_stale only catches
            // zero-content orphans). Production audit found ~10
            // `/tmp/.../tokenade-sem-*` rows from test runs that
            // wrote a real 2-file index then removed the tempdir.
            // prune_missing has no TTL guard — that's safe here
            // because evict-stale is the explicit user-invoked
            // maintenance entry point, not a background sweep.
            report_pruned(
                "semantic_db",
                tokenade_core::semantic_db::prune_missing(&conn),
                &mut t.rows,
            );
        }
        Err(e) => eprintln!("  semantic_db: open failed: {e}"),
    }
}

fn sweep_index_db(t: &mut SweepTally) {
    match tokenade_core::index_db::open() {
        Ok(conn) => {
            report_evicted(
                "index_db",
                "zero-symbol orphan rows dropped",
                "nothing to evict",
                tokenade_core::index_db::evict_stale(&conn),
                &mut t.rows,
            );
            // Mirror of the semantic_db prune above — same rationale.
            report_pruned(
                "index_db",
                tokenade_core::index_db::prune_missing(&conn),
                &mut t.rows,
            );
            // OPP-2 — cold-prune (60-day-untouched projects) + LRU size
            // cap (default 64 MiB, env TOKENADE_INDEX_MAX_BYTES). An
            // evicted project re-indexes cheaply on its next save.
            match tokenade_core::index_db::evict_to_size_cap(&conn) {
                Ok((n, freed)) if n > 0 => {
                    println!(
                        "  index_db: {n} cold/over-cap project row(s) dropped ({} freed)",
                        human_bytes(freed)
                    );
                    t.rows += n;
                }
                Ok(_) => println!("  index_db: under size cap, no cold projects"),
                Err(e) => eprintln!("  index_db size-cap: {e}"),
            }
        }
        Err(e) => eprintln!("  index_db: open failed: {e}"),
    }
}

fn sweep_projects_registry(t: &mut SweepTally) {
    // L65 — projects-registry.txt prune. The file is append-only; stale
    // entries (paths that no longer exist) silently get filtered when read but
    // stay in the file forever. Production audit (#448): 145 lines, mostly
    //
    // NOTE (2026-08-04): this comment used to point at `register_project` in a
    // `tokenade-mcp` crate. That crate is gone, and with it the only writer —
    // nothing in the tree appends to this file any more (the dogfood copy has
    // not grown since 2026-07-11). The pruner and `report.rs`'s per-repo
    // settings scan still read it, so they now work only on installs old
    // enough to have entries.
    // /tmp/claude-1000/tokenade-mcp-test-* orphans from MCP test
    // runs that registered project rows but were never cleaned up.
    {
        let dropped = tokenade_core::platform::prune_projects_registry();
        if dropped > 0 {
            println!("  projects-registry: {dropped} stale path(s) dropped");
            t.rows += dropped;
        } else {
            println!("  projects-registry: nothing to evict");
        }
    }
}

fn sweep_sessions(t: &mut SweepTally) {
    // Per-session scratch files (webfetch cache / websearch seen / overhead
    // counters) under ~/.tokenade/sessions/ — 7-day TTL, current session safe.
    {
        let n = tokenade_core::gain::evict_stale_sessions(tokenade_core::gain::SESSIONS_TTL_SECS);
        if n > 0 {
            println!("  sessions: {n} stale per-session file(s) removed (TTL)");
            t.rows += n;
        } else {
            println!("  sessions: nothing TTL-stale");
        }
    }
}

fn sweep_transcript_cache(t: &mut SweepTally) {
    // The per-file transcript extraction keeps one entry per session file.
    // A session that is deleted or a project that is removed would otherwise
    // leave its rows in the cache for good — small per entry, unbounded over
    // time, and describing a corpus that no longer exists.
    let n = {
        let home =
            tokenade_core::platform::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        tokenade_core::transcript_cache::with_shared(&home, |c| c.forget_missing())
    };
    if n > 0 {
        println!("  transcript cache: {n} entry(ies) for files that are gone");
        t.rows += n;
    } else {
        println!("  transcript cache: every entry still has its file");
    }
}

fn sweep_image_reads(t: &mut SweepTally) {
    // Per-project image-read cache (<cwd>/.tokenade/reads/): downscaled +
    // full-res copies, 7-day TTL. Scoped to the current project dir.
    if let Ok(cwd) = std::env::current_dir() {
        let n = tokenade_core::image_downscale::evict_stale_reads(
            &cwd,
            tokenade_core::image_downscale::READS_TTL_SECS,
        );
        if n > 0 {
            println!("  image reads: {n} stale downscale/full-res copy(ies) removed (TTL)");
            t.rows += n;
        } else {
            println!("  image reads: nothing TTL-stale");
        }
    }
}

fn sweep_bypass_counter(t: &mut SweepTally) {
    // #856 — bypass-counter prune. See `prune_stale_bypass_counter`.
    if let Some(home) = tokenade_core::platform::home_dir() {
        let now = tokenade_core::platform::now_unix_secs();
        match prune_stale_bypass_counter(&home, now) {
            Ok(true) => {
                println!("  bypass-counter: stale window dropped");
                t.rows += 1;
            }
            Ok(false) => {}
            Err(e) => eprintln!("  bypass-counter: {e}"),
        }
    }
}

fn sweep_disclose_stash(t: &mut SweepTally) {
    // L64 — stash files (progressive-disclosure cache).
    report_evicted(
        "disclose stash",
        "stashed file(s) removed (TTL)",
        "nothing TTL-stale",
        tokenade_core::disclose::evict_stale(),
        &mut t.rows,
    );
    // #1031 — LRU size-cap eviction (oldest-mtime first) to bound the
    // stash dir in burst scenarios where TTL alone hasn't kicked in.
    // Cap defaults to STASH_MAX_DIR_BYTES (2 GiB), env-overridable via
    // TOKENADE_STASH_MAX_DIR_BYTES.
    match tokenade_core::disclose::evict_to_size_cap() {
        Ok((n, freed)) if n > 0 => {
            println!(
                "  disclose stash: {n} stashed file(s) removed (LRU, {} freed)",
                human_bytes(freed),
            );
            t.rows += n;
            // Plain files unlinked from disk — there is no database here for
            // the VACUUM loop below to pick these bytes up from.
            t.bytes += freed;
        }
        Ok(_) => println!("  disclose stash: under size cap"),
        Err(e) => eprintln!("  disclose stash (size cap): {e}"),
    }
}

fn vacuum_databases(t: &mut SweepTally) {
    // VACUUM the SQLite databases to reclaim free pages from the
    // deletes above. SQLite keeps deleted pages in the freelist by
    // default — `VACUUM` rewrites the DB without the gaps. Run
    // unconditionally (cheap when the freelist is small; saves
    // 10s of MB after the orphan sweeps in #446/#447).
    // ONE entry per store this command sweeps, and the label comes from the
    // store's own `db_path` rather than from a string written beside it.
    //
    // The loop named three of the five, so `file_search.db` and
    // `fold_store.db` — the two `sweep_chunk_stores` deletes rows from a few
    // calls above — kept the pages this very command had just freed. Another
    // sweep does reach them, `vacuum_bloated_stores` on the once-a-day
    // session-start path, but only past a 4 MiB waste floor: measured on the
    // development machine, file_search.db sat at 136 of 407 pages free, a
    // third of the file and 557 KB, under the floor, with `strings` reading
    // source code out of those pages from chunks deleted weeks ago.
    //
    // `file_search.db` must vacuum through its OWN connection: it carries
    // sqlite-vec virtual tables that a bare connection cannot rebuild.
    for (path, (before, after)) in [
        (
            tokenade_core::dedup_ref::db_path(),
            tokenade_core::dedup_ref::vacuum_db(),
        ),
        (
            tokenade_core::index_db::db_path(),
            tokenade_core::index_db::vacuum_db(),
        ),
        (
            tokenade_core::semantic_db::db_path(),
            tokenade_core::semantic_db::vacuum_db(),
        ),
        (
            tokenade_core::file_search::db_path(),
            tokenade_core::file_search::vacuum_db(),
        ),
        (
            tokenade_core::fold_store::db_path(),
            tokenade_core::fold_store::vacuum_db(),
        ),
    ] {
        if before > after {
            let label = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let freed = before - after;
            println!(
                "  vacuumed {label}: {} → {} ({} freed)",
                human_bytes(before),
                human_bytes(after),
                human_bytes(freed)
            );
            t.bytes += freed;
        }
    }
}

fn sweep_models(drop_models: bool, t: &mut SweepTally) {
    // Local-model weights. Swept by NOTHING, and on a machine that has ever
    // run a local agent they are, by an order of magnitude, the largest thing
    // tokenade holds — MEASURED at 19.9 GB of the 20.6 GB total on the
    // development machine.
    //
    // Deliberately NOT on a TTL like everything above. A model is not a cache:
    // dropping a 2 GB file because it went a week unused costs the user a 2 GB
    // download the next time they ask for it, and they never asked for it to
    // go. So the default run only SAYS how much is there — the number nothing
    // else in the product would tell them — and `--models` is the explicit way
    // to reclaim it.
    let models = tokenade_core::platform::home_dir()
        .map(|h| h.join(".tokenade").join("models"))
        .filter(|p| p.is_dir());
    if let Some(dir) = models {
        let bytes: u64 = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        if drop_models {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {
                    println!("  models: removed ({} freed)", human_bytes(bytes));
                    t.bytes += bytes;
                }
                Err(e) => eprintln!("  models: {e}"),
            }
        } else if bytes > 1_073_741_824 {
            println!(
                "  models: {} held and never swept — `tokenade evict-stale --models` \
                 removes it (only local agents use these weights; they re-download on demand)",
                human_bytes(bytes)
            );
        }
    }
}

pub(crate) fn cmd_evict_stale(rest: &[String]) -> i32 {
    // #656 — until this iteration the signature was `()` (rest
    // silently discarded at dispatch). A user typing `evict-stale
    // --dryrun` (expecting a preview, mirroring uninstall/install)
    // got the destructive sweep + VACUUM with no warning. The TTL is
    // hard-coded, but `--models` IS a real flag: it deletes the local
    // model weights outright. This comment said \"no real flags\" three
    // lines above the code that reads one, and the help text said \"No
    // flags\" — so the only way to reclaim 19.9 GB was denied by both
    // places a user would look for it.
    let drop_models = rest.iter().any(|a| a == "--models");
    if let Err(code) = validate_simple_bool_flags(rest, "evict-stale", EVICT_STALE_FLAGS) {
        return code;
    }
    let mut t = SweepTally::default();
    println!("[tokenade evict-stale] sweeping all caches with 7-day TTL");
    sweep_dedup_ref(&mut t);
    sweep_chunk_stores(&mut t);
    sweep_semantic_db(&mut t);
    sweep_index_db(&mut t);
    sweep_projects_registry(&mut t);
    sweep_sessions(&mut t);
    sweep_image_reads(&mut t);
    sweep_transcript_cache(&mut t);
    sweep_bypass_counter(&mut t);
    sweep_disclose_stash(&mut t);
    // #499 — prune gain.jsonl noise rows. Catches:
    //   * historic zero-saved rows from before per-call-site guards
    //     shipped (#411/#417/#423/#424/#444),
    //   * fresh leaks from long-running MCP processes still on a
    //     pre-guard binary (the user has no easy way to restart them
    //     mid-session).
    // Preserves code-intel before==after rows because the dashboard
    // heuristic expands their baselines at read time.
    report_evicted(
        "gain.jsonl",
        "noise row(s) dropped",
        "nothing to evict",
        tokenade_core::gain::prune_noise(),
        &mut t.rows,
    );
    vacuum_databases(&mut t);
    sweep_models(drop_models, &mut t);
    println!(
        "Total: {} row(s) evicted, {} reclaimed across caches.",
        t.rows,
        human_bytes(t.bytes)
    );
    0
}
