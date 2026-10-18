//! **Build the index, write it, read it back, and bring it up to date.**
//!
//! Everything touching PERSISTENCE. The incremental rebuild lives here too:
//! deciding what changed is the same question as deciding what was recorded,
//! and separating them would make the two diverge.

use super::*;

impl Index {
    /// Walk `root`, extract symbols and imports from every supported file.
    ///
    /// Parallelised with rayon: the directory walk is single-threaded (cheap)
    /// but the per-file regex extraction is the hot path, so we fan that
    /// across all CPU cores. On a 4-core laptop, a 10 k-file repo drops from
    /// ~3 s single-threaded to ~700 ms.
    pub fn build(root: &Path) -> std::io::Result<Index> {
        use rayon::prelude::*;
        use std::collections::HashSet;

        // Load .gitignore at the project root (best-effort — missing
        // file yields an empty matcher).
        // Per-directory ignore stack: git reads a `.gitignore` in EVERY
        // directory and a deeper one overrides a shallower one.
        let gi = crate::gitignore::IgnoreStack::at_root(root);

        // Track real (resolved) paths we've already entered to catch
        // symlink loops. Without this, a `vendor/foo` → `vendor`
        // self-link sends the walker into an infinite descent.
        let mut visited: HashSet<PathBuf> = HashSet::new();
        if let Ok(canon) = fs::canonicalize(root) {
            visited.insert(canon);
        }

        let mut paths: Vec<(PathBuf, String)> = Vec::new();
        let mut stack = vec![(root.to_path_buf(), gi.clone())];
        while let Some((dir, gi)) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let rel = path.strip_prefix(root).unwrap_or(&path);
                if path.is_dir() {
                    // A symlinked directory is not part of the project. git
                    // records it as a link (mode 120000) and never descends
                    // it, and this walker already follows git's ignore rules,
                    // so descending one puts files the project does not
                    // contain into answers about the project — and costs
                    // whatever the link points at. MEASURED: a one-file repo
                    // carrying a link to a sibling tree went from 127 ms to
                    // 12 857 ms and indexed 1 490 files `git ls-files` does
                    // not list. `file_type()` is the entry's OWN type (lstat);
                    // `path.is_dir()` above has already followed the link.
                    if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                        continue;
                    }
                    if should_skip_dir(&name, !gi.is_empty()) {
                        continue;
                    }
                    if gi.matches(rel, &name, true) {
                        continue;
                    }
                    // Symlink-cycle guard: canonicalize, skip if seen.
                    if let Ok(canon) = fs::canonicalize(&path) {
                        if !visited.insert(canon) {
                            continue;
                        }
                    }
                    let child = gi.descend(rel, &path);
                    stack.push((path, child));
                } else {
                    // Apply gitignore rule for files too — covers `*.log`
                    // and other file-shaped excludes.
                    if gi.matches(rel, &name, false) {
                        continue;
                    }
                    let ext_owned = path.extension().and_then(|e| e.to_str()).map(String::from);
                    // Extensionless config files (Makefile, Dockerfile,
                    // Rakefile, Gemfile, Procfile, Caddyfile, Justfile,
                    // Vagrantfile, Brewfile, conf/routes). Index them
                    // with a synthetic "ext" so symbol_find can find
                    // them. The extractor for these falls back to
                    // generic regex; the win is just *visibility*.
                    let synth_ext = match name.as_str() {
                        "Makefile" | "GNUmakefile" => Some("makefile"),
                        "Dockerfile" => Some("dockerfile"),
                        "Rakefile" => Some("rakefile"),
                        "Gemfile" => Some("gemfile"),
                        "Procfile" => Some("procfile"),
                        "Caddyfile" => Some("caddyfile"),
                        "Justfile" | "justfile" => Some("justfile"),
                        "Vagrantfile" => Some("vagrantfile"),
                        "Brewfile" => Some("brewfile"),
                        _ => None,
                    };
                    let chosen_ext = ext_owned.or_else(|| synth_ext.map(String::from));
                    if let Some(ext) = chosen_ext {
                        if lang_for(&ext).is_some()
                            || crate::treesitter::hosts_embedded_script(&ext)
                            || synth_ext.is_some()
                        {
                            // Skip oversized files (minified bundles,
                            // generated artefacts, vendored blobs). 1 MiB
                            // covers every hand-written source we care
                            // about; anything bigger pollutes the symbol
                            // index with noise and pushes legitimate
                            // matches down the ranking. `metadata()` is
                            // cheap — one stat() per file — and far
                            // faster than reading the bytes only to
                            // discard them.
                            // `fs::metadata` FOLLOWS a symlink; the
                            // `DirEntry::metadata` used here did not, so a
                            // link to an oversized file measured as the LINK
                            // (a few bytes) and slipped past this cap — while
                            // `rebuild_incremental`, which stats the target,
                            // applied it. The same query then answered
                            // differently before and after any edit. The size
                            // that matters is the size of what gets read.
                            if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_INDEX_FILE_BYTES) {
                                continue;
                            }
                            paths.push((path, ext));
                        }
                    }
                }
            }
        }
        // Visibility: tell the caller (CLI / MCP / user) what we're about
        // to do so a several-second index build doesn't look like a hang.
        // Only emit when there's real work — under 50 files is sub-100ms.
        let started = std::time::Instant::now();
        let announce = paths.len() >= 50;
        if announce {
            crate::progress!(
                "[tokenade] indexing {} source file(s) (one-time, parallelised)…",
                paths.len()
            );
        }
        // Extract per-file results in parallel, then reduce into a single
        // Index. This keeps Index::index_file as a no-op shell that the
        // incremental rebuild path can still reuse single-threaded.
        let partials: Vec<FilePartial> = paths
            .par_iter()
            .filter_map(|(p, ext)| extract_file(root, p, ext))
            .collect();
        if announce {
            crate::progress!(
                "[tokenade] indexed {} file(s) in {}ms.",
                paths.len(),
                started.elapsed().as_millis()
            );
        }

        let mut idx = Index {
            root: root.display().to_string(),
            ..Default::default()
        };
        for fp in partials {
            idx.merge_parse(fp);
        }
        idx.symbols.sort_by(|a, b| a.name.cmp(&b.name));
        idx.built_by = env!("CARGO_PKG_VERSION").to_string();
        Ok(idx)
    }

    /// Fold one SUCCESSFUL parse into the index: the single declarer of what
    /// "this file is now indexed" means, shared by the parallel full build and
    /// the incremental rebuild, which had two copies of it that differed.
    ///
    /// It only ADDS. Replacing a file already in the index means wiping its
    /// previous symbols, imports and call edges first — the incremental
    /// rebuild does that in one place, and only once it holds a parse that
    /// actually succeeded. Wiping here instead would cost the full build a
    /// scan of every edge per file for nothing.
    fn merge_parse(&mut self, fp: FilePartial) {
        self.symbols.extend(fp.symbols);
        if !fp.imports.is_empty() {
            self.imports.insert(fp.rel.clone(), fp.imports);
        }
        if !fp.calls.is_empty() {
            self.calls.extend(fp.calls);
        }
        if fp.mtime > 0 {
            self.mtimes.insert(fp.rel, fp.mtime);
        }
        self.files_indexed += 1;
    }

    /// Incrementally rebuild starting from `prev`: walk `root` but only
    /// re-parse files whose on-disk mtime is newer than the value in
    /// `prev.mtimes`. Files in `prev` that no longer exist are dropped from
    /// symbols/imports/mtimes. New files are indexed fresh.
    ///
    /// Read a previously-persisted index from `<root>/.tokenade/index.json`
    /// (or the legacy `.sts/index.json` for users who haven't yet had the
    /// rename migration run). Returns `None` if the file is missing or
    /// malformed — never throws.
    /// Load the project's index. Since L56 storage is centralised in
    /// `~/.tokenade/index.db` keyed by project path; legacy per-project
    /// JSON files (`.tokenade/index.json`, `.sts/index.json`) are
    /// consulted only as a fallback for projects that haven't migrated
    /// yet (`project_scaffold::ensure` handles migration on first MCP
    /// touch).
    pub fn load(root: &Path) -> Option<Index> {
        Self::load_with_origin(root).map(|(idx, _)| idx)
    }

    /// As [`Self::load`], plus whether the index came from the GLOBAL db
    /// rather than a legacy per-project JSON file.
    ///
    /// [`Self::ensure_fresh`] needs the distinction: it may skip persisting an
    /// index nothing changed, and that is only safe when the global db already
    /// holds it. A legacy JSON read must still be written through, or the
    /// migration to the global db would never happen on a repository whose
    /// files never change.
    fn load_with_origin(root: &Path) -> Option<(Index, bool)> {
        // 1. Global DB (the post-L56 home).
        if let Ok(conn) = crate::index_db::open() {
            if let Ok(Some((blob, stamp))) = crate::index_db::load_blob_with_stamp(&conn, root) {
                if let Ok(idx) = serde_json::from_slice::<Index>(&blob) {
                    if idx.built_by == env!("CARGO_PKG_VERSION") {
                        // Reading an index IS using the project. Eviction
                        // orders by `last_updated`, and since the no-op save
                        // was skipped nothing else moves it — see
                        // `index_db::touch`. Rate-limited: the mark only needs
                        // to be right to the hour, and the write is not free.
                        if crate::index_db::touch_is_due(stamp) {
                            let _ = crate::index_db::touch(&conn, root);
                        }
                        return Some((idx, true));
                    }
                }
            }
        }
        // 2. Legacy per-project JSON, kept for read-through until the
        // scaffold migrator runs.
        for candidate in [
            root.join(".tokenade").join("index.json"),
            root.join(".sts").join("index.json"),
        ] {
            if let Ok(s) = std::fs::read_to_string(&candidate) {
                if let Ok(idx) = serde_json::from_str::<Index>(&s) {
                    if idx.built_by == env!("CARGO_PKG_VERSION") {
                        return Some((idx, false));
                    }
                }
            }
        }
        None
    }

    /// Rewrite every stored path to be relative to `cwd` instead of the
    /// project root.
    ///
    /// The index is keyed on the PROJECT, so its paths
    /// are root-relative — but a command runs in a directory, and both sides
    /// of the conversation use that directory as their base. Before this, a
    /// `query` run from `crates/core/src` answered
    /// `crates/cli/src/main.rs:2252`, which does not exist from there: the
    /// agent's very next `Read` fails on the path `query` just handed it. The
    /// input side had the mirror defect — `impact index.rs` in the directory
    /// holding `index.rs` answered "No such file or directory".
    ///
    /// A no-op when `cwd` IS the root, which is the common case.
    pub fn rebase_to(&mut self, cwd: &Path) {
        let Some(r) = crate::platform::PathRebase::new(Path::new(&self.root), cwd) else {
            return;
        };
        for sym in &mut self.symbols {
            sym.file = r.apply(&sym.file);
        }
        for c in &mut self.calls {
            c.file = r.apply(&c.file);
        }
        self.imports = std::mem::take(&mut self.imports)
            .into_iter()
            .map(|(k, v)| (r.apply(&k), v))
            .collect();
        self.mtimes = std::mem::take(&mut self.mtimes)
            .into_iter()
            .map(|(k, v)| (r.apply(&k), v))
            .collect();
        // `root` is the BASE every path in this index is relative to, and the
        // two must move together: `render_with_context` opens `root.join(file)`
        // to show source, and leaving `root` at the project root made it read
        // `/p/inner.rs` for a file that is at `/p/a/b/inner.rs`. Measured
        // straight after path rebasing shipped: `query --context` from a subdirectory
        // answered `(file unreadable)` about a file sitting right there. The
        // `rebased` flag stopped SAVES; it did not stop READS.
        self.root = cwd.display().to_string();
        self.rebased = true;
    }

    /// Fetch the call graph, which [`Self::load`] deliberately leaves behind.
    ///
    /// Every command used to pay for it: on a 3 980-file repository the graph
    /// is 39.7 MB of a 51.0 MB index and 63.7 ms of the 106 ms spent parsing
    /// it, while `semantic`, `map` and `skeleton` — 84 % of code-intel
    /// invocations on this machine's ledger — never look at a single edge.
    ///
    /// Idempotent, and a no-op once the graph is in hand. An index written
    /// before the split carries its edges inside the main blob and arrives
    /// already loaded.
    pub fn load_calls(&mut self, root: &Path) -> std::io::Result<()> {
        if !self.calls.is_empty() {
            return Ok(());
        }
        let _p = crate::profile::phase("index:load_calls");
        let conn =
            crate::index_db::open().map_err(|e| std::io::Error::other(format!("index db: {e}")))?;
        let blob = crate::index_db::load_calls_blob(&conn, root)
            .map_err(|e| std::io::Error::other(format!("index db: {e}")))?;
        if let Some(blob) = blob {
            self.calls = serde_json::from_slice(&blob).unwrap_or_default();
        }
        Ok(())
    }

    /// Persist this index to the global DB at `~/.tokenade/index.db`.
    /// Best-effort: errors are reported but no caller currently needs
    /// them to be fatal.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        if self.rebased {
            // Persisting cwd-relative paths would poison the store for every
            // other directory. Refused by construction rather than by
            // convention — see `rebase_to`.
            crate::debug_log::log(
                "index",
                "refused_saving_rebased_index",
                serde_json::json!({ "root": root.display().to_string() }),
            );
            return Ok(());
        }
        let blob = serde_json::to_vec(self).unwrap_or_default();
        let calls_blob = serde_json::to_vec(&self.calls).unwrap_or_default();
        let conn = crate::index_db::open()
            .map_err(|e| std::io::Error::other(format!("index_db open: {e}")))?;
        crate::index_db::save_blob(
            &conn,
            root,
            &blob,
            &calls_blob,
            self.files_indexed,
            self.symbols.len(),
        )
        .map_err(|e| std::io::Error::other(format!("index_db save: {e}")))?;
        // The legacy per-project sidecars are MIGRATION artefacts: `load`
        // falls back to them only when the store has no row, and
        // `migrate_local_index` exists to import and DELETE them. Once the
        // store holds this project they are stale copies of the truth, and an
        // external reader cannot tell. `SemanticIndex::save` has nuked its own
        // pair since the store landed; this is the sibling that did not, and
        // `tokenade index` wrote the symbol one back on every run — measured
        // on this repository at **22.2 MB inside the project tree**, four
        // times the 5.5 MB the store keeps for the same index.
        //
        // Removed only after the store write SUCCEEDS, so a failure leaves the
        // only remaining copy where `load` can still find it.
        let _ = fs::remove_file(root.join(".tokenade").join("index.json"));
        let _ = fs::remove_file(root.join(".sts").join("index.json"));
        Ok(())
    }

    /// Load + incremental-refresh + persist + return — the single helper
    /// every search path (`symbol_find`, `impact_of`, `structure_map`,
    /// `skeleton`, `tokenade query/impact/map`) should call instead of
    /// rebuilding from scratch every time.
    ///
    /// Behaviour, in order:
    ///   1. `load`: read the persisted index if any
    ///   2. `rebuild_incremental` against current cwd — touches only files
    ///      whose `mtime` is newer than what we had cached. Cold path on a
    ///      cwd with no cache falls through to a full [`build`].
    ///   3. `save`: persist so the next call sees a fresh cache
    ///   4. Return the up-to-date `Index`
    ///
    /// On a project where nothing has changed, this is sub-millisecond
    /// (one fs read + one stat per file). On a project where 1 file
    /// changed, it re-parses just that file. Full rebuild only happens
    /// when no prior cache exists.
    pub fn ensure_fresh(root: &Path) -> std::io::Result<Index> {
        let _p = crate::profile::phase("index:ensure_fresh");
        let loaded = {
            let _p = crate::profile::phase("index:load");
            Index::load_with_origin(root)
        };
        // Whether the index we are about to return differs from the one on
        // disk. A read-only question on an unchanged repository must not
        // rewrite the whole index — see the note on the save below.
        let mut dirty = true;
        let idx = match loaded {
            Some((prev, from_global_db)) if !prev.mtimes.is_empty() => {
                // Auto-detect changes via mtime diff. The `rebuild_incremental`
                // path itself emits the `[tokenade] indexing N files…` line
                // only when N ≥ 50 — so a single-file change stays silent.
                let _p = crate::profile::phase("index:rebuild_incremental");
                match Index::rebuild_incremental_reporting(&prev, root) {
                    Ok((i, changed)) => {
                        dirty = changed || !from_global_db;
                        i
                    }
                    // If incremental fails for any reason (corrupted cache,
                    // permission issue), fall back to a full build rather
                    // than returning stale data. Propagates the inner error
                    // if even that fails — never panics.
                    Err(_) => Index::build(root)?,
                }
            }
            _ => Index::build(root)?,
        };
        // Persist so we get warm-cache on the next call. Failure to
        // persist isn't an error for the caller — they got a fresh index
        // back; the cache file just isn't updated.
        //
        // Skipped when nothing changed, which is the common case: `query`,
        // `map` and `impact` ask a READ-ONLY question, and this used to
        // re-serialise the whole index and write it back every time. Measured
        // with `TOKENADE_PROFILE=1`, release build, on a 3 980-file Go
        // repository where not one file had moved: load 109 416 µs, rebuild
        // 48 916 µs, **save 73 821 µs** — 30 % of the index path spent
        // rewriting 51 MB that were already there, plus that much disk write
        // per query, forever.
        //
        // `!from_global_db` keeps the write when the index came from a legacy
        // per-project JSON: skipping it would strand the migration on a
        // repository whose files never change.
        if dirty {
            let _p = crate::profile::phase("index:save");
            let _ = idx.save(root);
        }
        Ok(idx)
    }

    /// Falls back to a full [`build`] when `prev` is empty (cold start) or
    /// has no mtime data (old v3 on-disk index loaded without `mtimes`).
    pub fn rebuild_incremental(prev: &Index, root: &Path) -> std::io::Result<Index> {
        Self::rebuild_incremental_reporting(prev, root).map(|(i, _)| i)
    }

    /// As [`Self::rebuild_incremental`], plus whether anything actually moved:
    /// a file dropped, or a file re-parsed. `false` means the returned index
    /// is content-identical to `prev`, which is what lets the caller skip
    /// persisting it.
    ///
    /// A full build is reported as changed — it produced the index from
    /// nothing, so there is nothing on disk it could match.
    pub fn rebuild_incremental_reporting(
        prev: &Index,
        root: &Path,
    ) -> std::io::Result<(Index, bool)> {
        if prev.mtimes.is_empty() || prev.files_indexed == 0 {
            return Index::build(root).map(|i| (i, true));
        }
        let mut idx = Index {
            root: root.display().to_string(),
            symbols: prev.symbols.clone(),
            imports: prev.imports.clone(),
            files_indexed: prev.files_indexed,
            mtimes: prev.mtimes.clone(),
            calls: prev.calls.clone(),
            built_by: prev.built_by.clone(),
            rebased: false,
        };
        // 1. Walk the tree, collecting current files + mtimes.
        let mut current: BTreeMap<String, (PathBuf, String, u64)> = BTreeMap::new();
        use std::collections::HashSet;
        let gi = crate::gitignore::IgnoreStack::at_root(root);
        let mut visited: HashSet<PathBuf> = HashSet::new();
        if let Ok(canon) = fs::canonicalize(root) {
            visited.insert(canon);
        }
        // Whether the ROOT itself could be opened. `current.is_empty()` cannot
        // stand in for this: `rm *.rs` leaves a perfectly readable root with
        // nothing in it, and that is an ordinary rebuild.
        let mut root_readable = false;
        let mut stack = vec![(root.to_path_buf(), gi.clone())];
        while let Some((dir, gi)) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if dir == root {
                root_readable = true;
            }
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let rel_path = path.strip_prefix(root).unwrap_or(&path);
                if path.is_dir() {
                    // A symlinked directory is not part of the project. git
                    // records it as a link (mode 120000) and never descends
                    // it, and this walker already follows git's ignore rules,
                    // so descending one puts files the project does not
                    // contain into answers about the project — and costs
                    // whatever the link points at. MEASURED: a one-file repo
                    // carrying a link to a sibling tree went from 127 ms to
                    // 12 857 ms and indexed 1 490 files `git ls-files` does
                    // not list. `file_type()` is the entry's OWN type (lstat);
                    // `path.is_dir()` above has already followed the link.
                    if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                        continue;
                    }
                    if should_skip_dir(&name, !gi.is_empty()) {
                        continue;
                    }
                    if gi.matches(rel_path, &name, true) {
                        continue;
                    }
                    if let Ok(canon) = fs::canonicalize(&path) {
                        if !visited.insert(canon) {
                            continue;
                        }
                    }
                    let child = gi.descend(rel_path, &path);
                    stack.push((path, child));
                } else {
                    if gi.matches(rel_path, &name, false) {
                        continue;
                    }
                    let ext = path.extension().and_then(|e| e.to_str()).map(String::from);
                    if let Some(ext) = ext {
                        if lang_for(&ext).is_some()
                            || crate::treesitter::hosts_embedded_script(&ext)
                        {
                            // Same size cap as the full-build walker — see
                            // MAX_INDEX_FILE_BYTES rationale.
                            let md = fs::metadata(&path).ok();
                            if md.as_ref().is_some_and(|m| m.len() > MAX_INDEX_FILE_BYTES) {
                                continue;
                            }
                            let rel = rel_path_string(&path, root);
                            let mtime = md.as_ref().map(file_fingerprint).unwrap_or(0);
                            current.insert(rel, (path, ext, mtime));
                        }
                    }
                }
            }
        }
        // Seeing NOTHING is not evidence of absence, it is evidence of not
        // having looked. The walk above answers `continue` on an unreadable
        // directory, so a root that is momentarily gone — a detached worktree,
        // an unmounted share, a directory swapped under us, a `read_dir` that
        // simply failed — produces an empty `current`, every known file is
        // then classified DELETED, and this returns a perfectly successful
        // rebuild holding zero files. `query`, `map` and `impact` would then
        // answer "not found" about symbols that DO exist: the false-absence
        // class, arriving through the index rather than
        // through a fold.
        //
        // The discriminator is whether the ROOT could be opened, NOT whether
        // the walk produced files. The first version of this guard used the
        // latter and refused a legitimate case immediately: `rm *.rs` leaves a
        // readable root holding nothing, the index should simply drain, and a
        // guard that blocks it would pin a stale index forever — the opposite
        // failure, and the harder one to notice.
        //
        // Mined from `jcodemunch-mcp`'s "a campaign that saw nothing must not
        // certify everything": their discovery compared only for GROWTH, so a
        // vanished source root read as "nothing drifted" and stamped the
        // target generation having re-parsed nothing.
        let mut changed = false;
        // The graph is not in `prev` unless somebody asked for it, and both
        // mutation sites below edit it in place. Loading it at the first
        // change keeps the read-only path free while making the write path
        // whole: merging new edges into an EMPTY base would persist a graph
        // truncated to whatever changed. A load failure propagates, and
        // `ensure_fresh` answers it with a full rebuild rather than saving a
        // partial one.
        let mut calls_ready = false;
        macro_rules! ensure_calls {
            ($idx:expr) => {
                if !calls_ready {
                    $idx.load_calls(root)?;
                    calls_ready = true;
                }
            };
        }
        if !root_readable && prev.files_indexed > 0 {
            crate::debug_log::log(
                "index",
                "refused_empty_rebuild",
                serde_json::json!({
                    "root": root.display().to_string(),
                    "prev_files": prev.files_indexed,
                }),
            );
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "refusing to replace an index of {} file(s) with an empty one: {} could not be read at all — the root is gone, unreadable, or was swapped mid-walk",
                    prev.files_indexed,
                    root.display()
                ),
            ));
        }
        // 2. Drop files that no longer exist.
        let prev_files: Vec<String> = idx.mtimes.keys().cloned().collect();
        for f in &prev_files {
            if !current.contains_key(f) {
                changed = true;
                ensure_calls!(idx);
                idx.symbols.retain(|s| s.file != *f);
                // A file's call edges leave with it. An edit already
                // drops a re-parsed file's previous edges just above, so
                // only the deletion path was not handled: every file ever
                // removed left its outbound edges in the graph forever, and a
                // rename is a deletion. Invisible until `query` started
                // printing `called by:` — then it names callers in
                // files that no longer exist.
                idx.calls.retain(|c| c.file != *f);
                idx.imports.remove(f);
                idx.mtimes.remove(f);
                idx.files_indexed = idx.files_indexed.saturating_sub(1);
            }
        }
        // 3. For each current file, re-parse if its change fingerprint differs
        //    from the persisted one. Inequality (not `<=`) so a same-second edit
        //    (fingerprint folds size) AND an mtime that moved BACKWARD (older
        //    revision checked out) both re-index — a `<=` test skipped both.
        for (rel, (path, ext, mtime)) in current {
            let prev_mtime = idx.mtimes.get(&rel).copied().unwrap_or(0);
            if mtime == prev_mtime {
                continue; // unchanged — keep cached symbols/imports as-is
            }
            ensure_calls!(idx);
            // Parse FIRST, wipe SECOND. The order is the whole point: a parse
            // that FAILS is not an extraction to nothing.
            //
            // This used to wipe the file's symbols, decrement the file count
            // and then call a re-index that silently did nothing when
            // `extract_file` returned `None` — a file that became unreadable
            // (re-saved in a non-UTF-8 encoding, swapped mid-walk, permissions
            // changed) had every symbol deleted, and `query` then answered
            // "no symbol matching" about a function sitting two lines into a
            // file that is right there. Measured on a two-file repository: one
            // file turned Latin-1, `query beta_two` went from naming it to
            // denying it, `file_count` read 1 for 2 files and `symbol_count` 2
            // for 4 — and because the fingerprint was never recorded either,
            // EVERY subsequent read-only `query` re-entered this branch, set
            // `changed`, and rewrote the whole index (the no-op-save defect, back
            // permanently, for any repository holding one such file).
            //
            // Mined from `graphify` a8852e8: a run that silently lost content
            // to an unparseable chunk was treated as complete, so it could
            // overwrite the existing graph with a smaller one.
            let parsed = extract_file(root, &path, &ext);
            match parsed {
                Some(fp) => {
                    changed = true;
                    // The three wipes an edit needs, together: symbols,
                    // imports, and the outbound call edges that would
                    // otherwise survive as ghosts.
                    idx.symbols.retain(|s| s.file != rel);
                    idx.imports.remove(&rel);
                    idx.calls.retain(|c| c.file != rel);
                    if idx.mtimes.contains_key(&rel) {
                        idx.files_indexed = idx.files_indexed.saturating_sub(1);
                    }
                    idx.merge_parse(fp);
                }
                None => {
                    // Keep the last known-good symbols: a stale answer is what
                    // `query` would have given before the edit, and it beats a
                    // confident denial. Record the fingerprint so the retry
                    // stops firing on every read-only command — any FURTHER
                    // change moves it again and re-attempts the file, so a
                    // transient failure still heals.
                    //
                    // A fingerprint of 0 means the walker could not stat the
                    // file. Storing that would make the entry match a fresh 0
                    // and freeze the file out of every future rebuild, so the
                    // entry is left alone and `changed` is NOT set: the file is
                    // re-attempted next run, but no read-only command rewrites
                    // the index for it.
                    if mtime > 0 {
                        idx.mtimes.insert(rel.clone(), mtime);
                        changed = true;
                    }
                    crate::debug_log::log(
                        "index",
                        "kept_last_good_symbols",
                        serde_json::json!({
                            "file": rel,
                            "fingerprint": mtime,
                        }),
                    );
                }
            }
        }
        // `prev` was already sorted by this same key, so re-sorting every
        // symbol to change nothing is the second half of the same waste the
        // skipped save closes.
        if changed {
            idx.symbols.sort_by(|a, b| a.name.cmp(&b.name));
        }
        idx.built_by = env!("CARGO_PKG_VERSION").to_string();
        Ok((idx, changed))
    }
}

/// L56 migrator. Import a legacy `<root>/.tokenade/index.json` (or
/// `.sts/index.json`) into the global `~/.tokenade/index.db`, then
/// delete the legacy file. Returns `Some(action_summary)` on
/// touch, `None` if there was nothing to migrate.
///
/// Best-effort: read/parse failures on the legacy file simply
/// remove it without importing (the next `ensure_fresh` call will
/// rebuild from source).
pub fn migrate_local_index(root: &Path) -> Option<String> {
    let candidates = [
        root.join(".tokenade").join("index.json"),
        root.join(".sts").join("index.json"),
    ];
    let mut migrated_files = 0usize;
    let mut deleted_files = 0usize;
    let mut imported = false;

    for path in &candidates {
        if !path.is_file() {
            continue;
        }
        let parsed = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Index>(&s).ok());
        if let Some(idx) = parsed {
            if !imported {
                if let Ok(conn) = crate::index_db::open() {
                    let blob = serde_json::to_vec(&idx).unwrap_or_default();
                    let calls_blob = serde_json::to_vec(&idx.calls).unwrap_or_default();
                    if crate::index_db::save_blob(
                        &conn,
                        root,
                        &blob,
                        &calls_blob,
                        idx.files_indexed,
                        idx.symbols.len(),
                    )
                    .is_ok()
                    {
                        migrated_files = idx.files_indexed;
                        imported = true;
                    }
                }
            }
        }
        // Either we imported it or it was corrupt — either way the
        // file goes (the global DB is now authoritative or the
        // rebuild will recreate it).
        if std::fs::remove_file(path).is_ok() {
            deleted_files += 1;
        }
    }

    if deleted_files == 0 {
        return None;
    }
    if imported {
        Some(format!(
            ".tokenade/index.json (migrated {migrated_files} indexed file(s) to ~/.tokenade/index.db)"
        ))
    } else {
        Some(
            ".tokenade/index.json (deleted legacy file — will rebuild on next ensure_fresh)".into(),
        )
    }
}
