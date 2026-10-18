//! **Disk housekeeping, and the migration of a cache.**
//!
//! An embedding model downloaded per project takes up the disk as many times
//! as there are projects; it moves once to a global location. The garbage
//! collector, for its part, does not run on every call — the temptation would
//! be to do so, and it would cost more than what it frees.

use super::*;

/// Ensure tokenade rules are present in the project at `root`.
///
/// Writes to:
///   1. `CLAUDE.md` + `AGENTS.md` (always — they're the canonical
///      cross-agent rules locations).
///   2. Agent-specific files for any agent we detect installed on
///      the machine (see [`detect_installed_agents`]).
///   3. `.gitignore` — appends `.tokenade/` so the per-project
///      cache/index/scaffold-flag don't accidentally get committed.
///      Only when the project IS a git repo (has `.git/`).
///
/// Idempotency: a `.tokenade/scaffold.flag` sentinel skips the
/// whole pass after the first successful write. Marker detection
/// inside each file is a second safety net so deleting the flag
/// doesn't cause re-append.
/// How often the opportunistic disk GC may run, per machine.
pub(super) const DISK_GC_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// Opportunistic 7-day TTL eviction + size caps across every cache DB, at most
/// once per [`DISK_GC_INTERVAL_SECS`]. Returns the number of rows/files evicted
/// (0 when throttled). Silent on errors; cheap when there's nothing to evict.
///
/// Owns its OWN throttle rather than riding the scaffold's flag. It used to run
/// piggybacked at the end of `ensure`, i.e. behind the scaffold.flag skip — so
/// on a normal machine (stable project, flag already current) it never ran
/// again after the first session, and every "cap" the cache code advertises was
/// dead letter: index.db was measured at 1.34x its 64 MiB cap. A time-based
/// throttle is the honest cadence for disk hygiene: it tracks WALL TIME, which
/// is what garbage accumulation actually tracks.
pub(super) fn maybe_run_disk_gc(root: &Path) -> usize {
    let Some(stamp) =
        crate::platform::home_dir().map(|h| h.join(".tokenade").join(".last_disk_gc"))
    else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let due = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .and_then(|t| {
            now.duration_since(t)
                .map_err(|_| std::io::ErrorKind::Other.into())
        })
        .map(|age| age.as_secs() >= DISK_GC_INTERVAL_SECS)
        .unwrap_or(true); // never run (or unreadable stamp) → run now
    if !due {
        return 0;
    }
    // Stamp BEFORE the work: if the GC is slow or the process dies mid-pass,
    // concurrent/next sessions must not all pile onto the same eviction.
    if let Some(d) = stamp.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = crate::paths::write_atomic_repo_scoped(root, &stamp, "1");

    // OFF the session's critical path. The sweeps below take SECONDS on a
    // machine with a day's accumulation — measured 2026-08-28 on the
    // development box, `TOKENADE_PROFILE=1` on a real SessionStart event:
    // `sessionstart:scaffold_ensure` **3 285 931 µs**, against 38 056 µs for
    // the whole hook once the stamp was fresh. Every other phase in that run
    // was under 15 ms. It is once a day, and it is once a day IN FRONT OF the
    // agent's first turn.
    //
    // Same reasoning, same helper and same precedent as the reporter and the
    // updater: a detached THREAD dies with the process, and a hook lives
    // milliseconds while this needs seconds, so it must be a detached PROCESS.
    // The stamp is already written ABOVE, before the work, so the child — which
    // runs this same code on startup — finds the GC not due and cannot chain.
    //
    // `evict-stale` is this list plus the VACUUMs, and it is the documented
    // command for exactly this; running it removes the second copy of the
    // sweep list rather than adding a third.
    if let Ok(exe) = std::env::current_exe() {
        if crate::process_util::spawn_fully_detached_in(&exe, &["evict-stale"], Some(root)).is_ok()
        {
            return 0;
        }
    }
    // No usable binary path (or the spawn failed): do it inline rather than
    // not at all. Disk hygiene skipped forever is the worse failure.
    let mut evicted = 0usize;
    evicted += crate::dedup_ref::evict_stale(root).unwrap_or(0);
    // Bound dedup.db by total bytes (LRU). Age-only eviction above can't catch
    // a burst of unique large outputs inside the TTL.
    evicted += crate::dedup_ref::evict_to_size_cap().0;
    evicted += crate::file_search::evict_stale().unwrap_or(0);
    evicted += crate::fold_store::evict_stale().unwrap_or(0);
    if let Ok(conn) = crate::semantic_db::open() {
        evicted += crate::semantic_db::evict_stale(&conn).unwrap_or(0);
        evicted += crate::semantic_db::prune_missing(&conn).unwrap_or(0);
    }
    if let Ok(conn) = crate::index_db::open() {
        evicted += crate::index_db::evict_stale(&conn).unwrap_or(0);
        evicted += crate::index_db::prune_missing(&conn).unwrap_or(0);
        // Bound index.db (cold-prune + LRU size cap). Re-indexing an evicted
        // project is cheap and happens on its next save.
        evicted += crate::index_db::evict_to_size_cap(&conn)
            .map(|(n, _)| n)
            .unwrap_or(0);
    }
    evicted += crate::disclose::evict_stale().unwrap_or(0);
    evicted += crate::disclose::evict_to_size_cap()
        .map(|(n, _)| n)
        .unwrap_or(0);
    // Scratch dirs that otherwise have ZERO automatic cleanup: per-session
    // state under ~/.tokenade/sessions/ (webfetch cache / websearch seen-set /
    // overhead counters) and the per-project image-read cache under
    // <root>/.tokenade/reads/. Same 7-day TTL; the current session's files are
    // always fresh so they're never swept.
    evicted += crate::gain::evict_stale_sessions(crate::gain::SESSIONS_TTL_SECS);
    evicted +=
        crate::image_downscale::evict_stale_reads(root, crate::image_downscale::READS_TTL_SECS);
    // projects-registry.txt stale-path prune (was manual-only).
    evicted += crate::platform::prune_projects_registry();
    evicted
}

pub fn ensure(root: &Path) -> ScaffoldResult {
    // Disk hygiene FIRST, on its own throttle — deliberately ahead of every
    // early-return below. It used to be piggybacked at the END of this
    // function, which made it unreachable in normal use: the scaffold.flag
    // skip (and the quota-pause guard) return early, so the GC only ever ran
    // on a project's FIRST session or on a SCAFFOLD_VERSION bump. Measured
    // consequence: index.db reached 1.34x its own 64 MiB cap, i.e. the caps
    // the code advertises did not exist in practice. Eviction is not part of
    // the scaffold's "value" either, so a quota-paused machine must still get
    // its disk cleaned.
    let evicted = maybe_run_disk_gc(root);

    // A project retracted by a 1.x quota pause is restored by
    // `sync_gate_state(root, false)` (wrap / SessionStart); until then the
    // marker keeps this path from half-writing it.
    if retracted_marker(root).exists() {
        return ScaffoldResult {
            skipped: true,
            ..Default::default()
        };
    }
    let mut write_failed = false;
    let home = crate::platform::home_dir();
    // Before the flag skip: a project already up to date must still receive
    // (or heal) the out-of-repo lesson. A binary upgrade does not necessarily
    // open this repo on day one.
    let global_written = ensure_global_rules(home.as_deref());
    // Session opened in `$HOME`: `~/AGENTS.md` / `~/CLAUDE.md` are leftovers
    // that `ensure_global_rules` removes — rewriting them here reopened the
    // flip-flop on every session. The home is not a project.
    if home.as_deref().is_some_and(|h| h.starts_with(root)) {
        return ScaffoldResult {
            skipped: global_written.is_empty(),
            files_written: global_written
                .into_iter()
                .map(|p| p.display().to_string())
                .collect(),
        };
    }
    // Before the flag skip: a nested crate (cwd = crates/foo) still loads
    // CLAUDE.md / AGENTS.md next to the global lesson.
    let project_healed = heal_project_lectures(root);
    let flag = root.join(".tokenade").join("scaffold.flag");
    // Version-aware skip: only short-circuit when the flag exists
    // AND records a version >= the current SCAFFOLD_VERSION. An
    // older flag means the user was scaffolded with stale content
    // and we should re-run so the in-place upgrade splices the new
    // rules block.
    if let Ok(s) = std::fs::read_to_string(&flag) {
        let recorded = parse_flag_version(&s);
        // The VERSION alone is no longer enough: the written block also
        // carries the style line, which is a user setting and can change
        // without the product moving. Without this comparison,
        // `tokenade style coding` reaches ONLY never-scaffolded projects — the
        // class "a fix that never arrives in installs" that this file's header
        // already describes for the version.
        let style_changed = parse_flag_style(&s)
            .map(|recorded_style| recorded_style != crate::style::load(root).as_str())
            .unwrap_or(true);
        if recorded.unwrap_or(1) >= SCAFFOLD_VERSION && !style_changed {
            if project_healed.is_empty() {
                return ScaffoldResult {
                    skipped: true,
                    ..Default::default()
                };
            }
            return ScaffoldResult {
                skipped: false,
                files_written: project_healed
                    .into_iter()
                    .map(|p| p.display().to_string())
                    .collect(),
            };
        }
    }
    let mut written: Vec<String> = global_written
        .into_iter()
        .map(|p| p.display().to_string())
        .collect();

    let mut candidates: Vec<PathBuf> = PRIMARY_AGENT_FILES.iter().map(|n| root.join(n)).collect();
    candidates.extend(detect_installed_agents(root));
    let (targets, escaped) = resolve_rule_targets(root, &candidates);
    for t in &targets {
        let r = write_scaffold_to(root, &t.path, t.subagent_note);
        if r.is_none() {
            write_failed = true;
        }
        if r == Some(true) {
            written.push(t.label(root));
        }
    }
    // A rule file whose link leaves the repository is REFUSED, permanently and
    // on purpose (a cloned repo must not pick a file on your machine for us to
    // write). That is a decision, not a failure: counting it as one left the
    // scaffold flag unstamped and re-ran the whole pass on every session
    // forever. It is reported instead, so a user who linked their rules
    // somewhere finds out they have no rules rather than wondering.
    for link in &escaped {
        written.push(format!(
            "{} left alone (links outside the project — no rules written)",
            link.strip_prefix(root).unwrap_or(link).display()
        ));
    }

    // Gitignore step — keep .tokenade/ out of commits. Per-project
    // .gitignore (when git repo) AND the global ~/.config/git/ignore
    // (XDG default since git 1.7.6, no setup required for most users).
    // Either / both / neither may apply.
    if ensure_gitignored(root) {
        written.push(".gitignore (+.tokenade/)".to_string());
    }
    if ensure_global_gitignored() {
        written.push("~/.config/git/ignore (+.tokenade/, +*.tokenade-original)".to_string());
    }

    // One-shot migration for projects that downloaded the fastembed
    // model BEFORE we pinned the global cache. Move `<root>/
    // .fastembed_cache/` to `~/.cache/tokenade/fastembed/` (or just
    // delete the local copy if global already populated). Saves
    // ~100-700 MB per project, exactly what the user flagged
    // 2026-05-29.
    if let Some(action) = migrate_local_fastembed_cache(root) {
        written.push(action);
    }

    // L53: legacy per-project `<root>/.tokenade/dedup/` (file-based
    // §ref:HASH§ store) → global `~/.tokenade/dedup.db`. Content-
    // addressable, so cross-project hashes collide correctly and the
    // dedup is now session-wide instead of session+project-wide.
    if let Some(action) = crate::dedup_ref::migrate_local_store(root) {
        written.push(action);
    }

    // L54: legacy per-project `<root>/.tokenade/semantic.{bin,json}`
    // → global `~/.tokenade/semantic.db`. Closes the dual-write loop
    // L7 left open; without this the .bin kept reappearing on every
    // SemanticIndex::save.
    if let Some(action) = crate::semantic::migrate_local_semantic_bin(root) {
        written.push(action);
    }

    // L56: legacy per-project `<root>/.tokenade/index.json` (symbol
    // + import-graph index) → global `~/.tokenade/index.db` keyed by
    // project_path. Same pattern as L7 semantic_db.
    if let Some(action) = crate::index::migrate_local_index(root) {
        written.push(action);
    }

    // L57: legacy per-project `<root>/.tokenade/style.json` (response
    // style preference) → global `~/.tokenade/style.json`. Style is a
    // user pref, not a project trait — see style.rs config_path().
    if let Some(action) = crate::style::migrate_local_style(root) {
        written.push(action);
    }

    // L57: legacy per-project `<root>/.tokenade/gain.jsonl` (token
    // savings ledger) → global `~/.tokenade/gain.jsonl`. Already
    // global by design; per-project files are residual artifacts.
    if let Some(action) = crate::gain::migrate_local_ledger(root) {
        written.push(action);
    }

    if evicted > 0 {
        written.push(format!(
            "evicted {evicted} stale cache row(s)/file(s) (dedup/file_search/fold_store/semantic_db/index_db/disclose/sessions/reads/registry, 7-day TTL)"
        ));
    }

    // Audit: the flag was stamped even when a write FAILED (perms/disk),
    // silencing retries until the next SCAFFOLD_VERSION bump. Only stamp
    // when nothing failed; a failed pass retries next session.
    if write_failed {
        return ScaffoldResult {
            files_written: written,
            skipped: false,
        };
    }
    let _ = std::fs::create_dir_all(root.join(".tokenade"));
    let _ = crate::paths::write_atomic_repo_scoped(
        root,
        &flag,
        &format!(
            "version: {SCAFFOLD_VERSION}\nstyle: {}\nscaffolded: {}\n",
            crate::style::load(root).as_str(),
            written.join(", ")
        ),
    );
    ScaffoldResult {
        files_written: written,
        skipped: false,
    }
}

/// One-shot migration: if `<root>/.fastembed_cache/` exists, move
/// (or delete) it so the model isn't duplicated per project.
///
/// Decision matrix:
///   - global empty + local present → MOVE local → global (saves
///     disk + future projects share)
///   - global populated + local present → DELETE local (already
///     have the model centrally; the project copy is dead weight)
///   - global anything + local absent → no-op
///
/// Returns Some(<action description>) when we touched something,
/// None on no-op. Never destructive on the global cache — only the
/// project-local copy is mutated.
pub fn migrate_local_fastembed_cache(root: &Path) -> Option<String> {
    let local = root.join(".fastembed_cache");
    if !local.is_dir() {
        return None;
    }
    let global = global_fastembed_dir()?;
    let global_populated =
        global.is_dir() && std::fs::read_dir(&global).is_ok_and(|mut it| it.next().is_some());

    let local_size_mb = dir_size_mb(&local);

    if global_populated {
        // Just delete the local copy — global has the model already.
        if std::fs::remove_dir_all(&local).is_ok() {
            return Some(format!(
                ".fastembed_cache (deleted local — saved ~{local_size_mb} MB, global already has model)"
            ));
        }
        return None;
    }

    // Global empty — move local contents into it.
    if std::fs::create_dir_all(&global).is_err() {
        return None;
    }
    let moved = move_dir_contents(&local, &global).unwrap_or(0);
    let _ = std::fs::remove_dir_all(&local); // empty now
    if moved > 0 {
        Some(format!(
            ".fastembed_cache (moved {moved} item(s) to {} — saved ~{local_size_mb} MB per future project)",
            global.display()
        ))
    } else {
        None
    }
}

pub(super) fn global_fastembed_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("FASTEMBED_CACHE") {
        return Some(PathBuf::from(v));
    }
    crate::platform::cache_dir().map(|c| c.join("tokenade").join("fastembed"))
}

/// Rough directory size in MB (rounds down). Walks one level deep
/// for speed — fastembed lays out a flat-ish dir tree so this is
/// close enough for a "saved ~X MB" message.
pub(super) fn dir_size_mb(dir: &Path) -> u64 {
    fn walk(d: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(d) else {
            return 0;
        };
        let mut total = 0u64;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                total += e.metadata().map_or(0, |m| m.len());
            } else if p.is_dir() {
                total += walk(&p);
            }
        }
        total
    }
    walk(dir) / (1024 * 1024)
}

/// Move every entry inside `from` into `to`. Returns the count of
/// items moved successfully. Existing entries in `to` are NOT
/// overwritten (silent skip — global wins). Falls back to copy+
/// delete when rename fails (cross-device).
pub(super) fn move_dir_contents(from: &Path, to: &Path) -> std::io::Result<usize> {
    let mut moved = 0usize;
    for e in std::fs::read_dir(from)?.flatten() {
        let src = e.path();
        let name = match src.file_name() {
            Some(n) => n.to_owned(),
            None => continue,
        };
        let dst = to.join(&name);
        if dst.exists() {
            // Don't overwrite existing global content — assume
            // global has the canonical copy.
            let _ = std::fs::remove_dir_all(&src);
            let _ = std::fs::remove_file(&src);
            continue;
        }
        match std::fs::rename(&src, &dst) {
            Ok(()) => {
                moved += 1;
            }
            Err(_) => {
                // Cross-device — fall back to copy.
                if copy_recursive(&src, &dst).is_ok() {
                    let _ = std::fs::remove_dir_all(&src);
                    let _ = std::fs::remove_file(&src);
                    moved += 1;
                }
            }
        }
    }
    Ok(moved)
}

pub(super) fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)?.flatten() {
        copy_recursive(&e.path(), &dst.join(e.file_name()))?;
    }
    Ok(())
}
