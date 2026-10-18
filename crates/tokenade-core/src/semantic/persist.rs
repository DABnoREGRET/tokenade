//! **Writing and reading back the index**, and migrating it.
//!
//! The index lives in the global SQLite `~/.tokenade/semantic.db`, not next to
//! the project: an agent that switches repository must not pay for indexing
//! again.
//!
//! Hence `rebase_to`, and hence the migrator: versions ≤ L54 wrote one
//! `semantic.bin` per project. It is IMPORTED then deleted — leaving both in
//! place would make the old one answer a user who believes they are reading
//! the new one.

use super::*;

impl SemanticIndex {
    /// Persist the semantic index to the global SQLite at
    /// `~/.tokenade/semantic.db` (keyed by project root).
    ///
    /// Since L54 the legacy per-project `.bin` mirror is no longer
    /// written. The dual-write was masking L7's centralization goal
    /// and kept regenerating bloat the user repeatedly cleaned up.
    /// Migration of existing `.bin` files is handled by
    /// `migrate_local_semantic_bin` (called from project_scaffold)
    /// AND opportunistically by [`Self::load`] on cold reads.
    /// Rewrite every document path so it opens from `cwd` instead of the
    /// project root.
    ///
    /// `semantic` names files for the reader to open, exactly like `query`
    /// does, so it owes the same base — printing root-relative paths from a
    /// subdirectory hands back something the next `Read` cannot open.
    pub fn rebase_to(&mut self, root: &Path, cwd: &Path) {
        let Some(r) = crate::platform::PathRebase::new(root, cwd) else {
            return;
        };
        for d in &mut self.docs {
            d.file = r.apply(&d.file);
        }
        self.mtimes = std::mem::take(&mut self.mtimes)
            .into_iter()
            .map(|(k, v)| (r.apply(&k), v))
            .collect();
        self.rebased = true;
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        if self.rebased {
            // Persisting cwd-relative paths would poison the store for every
            // other directory — see `crate::index::Index::save`.
            crate::debug_log::log(
                "semantic",
                "refused_saving_rebased_index",
                serde_json::json!({ "root": root.display().to_string() }),
            );
            return Ok(());
        }
        let bytes = postcard::to_allocvec(self)
            .map_err(|e| std::io::Error::other(format!("postcard encode: {e}")))?;
        let conn = crate::semantic_db::open()
            .map_err(|e| std::io::Error::other(format!("semantic_db open: {e}")))?;
        crate::semantic_db::save_blob(&conn, root, &bytes, self.mtimes.len(), self.docs.len())
            .map_err(|e| std::io::Error::other(format!("semantic_db save: {e}")))?;
        // Belt-and-braces: nuke any legacy sidecars left over from
        // pre-L54 saves so external readers can't confuse them with
        // the current truth.
        let _ = fs::remove_file(root.join(".tokenade").join("semantic.bin"));
        let _ = fs::remove_file(root.join(".tokenade").join("semantic.json"));
        Ok(())
    }

    /// Load the index. Read order (2026-05-29):
    ///   1. **Global SQLite** (primary, post-L7).
    ///   2. **Per-project `semantic.bin`** (pre-L7 projects).
    ///   3. **Legacy `semantic.json`** (pre-v5 projects).
    ///
    /// When a per-project file is found, [`Self::save`] is called
    /// which both populates the global DB and removes the legacy
    /// sidecar. The NEXT load on this project goes through the fast
    /// path.
    pub fn load(root: &Path) -> Option<SemanticIndex> {
        if let Ok(conn) = crate::semantic_db::open() {
            if let Ok(Some(bytes)) = crate::semantic_db::load_blob(&conn, root) {
                if let Ok(idx) = postcard::from_bytes::<SemanticIndex>(&bytes) {
                    if idx.built_by == env!("CARGO_PKG_VERSION") {
                        return Some(idx);
                    }
                }
            }
        }
        let bin_path = root.join(".tokenade").join("semantic.bin");
        if let Ok(bytes) = fs::read(&bin_path) {
            if let Ok(idx) = postcard::from_bytes::<SemanticIndex>(&bytes) {
                if idx.built_by != env!("CARGO_PKG_VERSION") {
                    return None;
                }
                if let Err(e) = idx.save(root) {
                    crate::debug_log::log(
                        "semantic",
                        "migrate_save_failed",
                        serde_json::json!({ "root": root.to_string_lossy(), "error": e.to_string() }),
                    );
                }
                return Some(idx);
            }
        }
        let json_path = root.join(".tokenade").join("semantic.json");
        let s = fs::read_to_string(&json_path).ok()?;
        let idx: SemanticIndex = serde_json::from_str(&s).ok()?;
        if idx.built_by != env!("CARGO_PKG_VERSION") {
            return None;
        }
        if let Err(e) = idx.save(root) {
            crate::debug_log::log(
                "semantic",
                "migrate_save_failed",
                serde_json::json!({ "root": root.to_string_lossy(), "error": e.to_string() }),
            );
        }
        Some(idx)
    }
}

/// L54 migrator. Import a legacy per-project `semantic.bin` (or
/// `semantic.json`) into the global `~/.tokenade/semantic.db`, then
/// delete the legacy file. Returns `Some(action)` on touch, `None`
/// otherwise.
///
/// Best-effort: read/parse failures simply remove the legacy file
/// (the next `ensure_fresh`-style call will rebuild).
pub fn migrate_local_semantic_bin(root: &Path) -> Option<String> {
    let bin_path = root.join(".tokenade").join("semantic.bin");
    let json_path = root.join(".tokenade").join("semantic.json");
    let bin_exists = bin_path.is_file();
    let json_exists = json_path.is_file();
    if !bin_exists && !json_exists {
        return None;
    }
    let mut imported_docs = 0usize;
    let mut imported = false;

    if bin_exists {
        if let Ok(bytes) = fs::read(&bin_path) {
            if let Ok(idx) = postcard::from_bytes::<SemanticIndex>(&bytes) {
                imported_docs = idx.docs.len();
                // save() writes to the global DB AND removes the
                // sidecars; we don't need to remove_file explicitly.
                if idx.save(root).is_ok() {
                    imported = true;
                }
            }
        }
        let _ = fs::remove_file(&bin_path);
    }
    if json_exists {
        // Only fall back to JSON if .bin didn't import (rare).
        if !imported {
            if let Ok(s) = fs::read_to_string(&json_path) {
                if let Ok(idx) = serde_json::from_str::<SemanticIndex>(&s) {
                    imported_docs = idx.docs.len();
                    if idx.save(root).is_ok() {
                        imported = true;
                    }
                }
            }
        }
        let _ = fs::remove_file(&json_path);
    }
    if imported {
        Some(format!(
            ".tokenade/semantic.{{bin,json}} (migrated {imported_docs} doc(s) to ~/.tokenade/semantic.db)"
        ))
    } else {
        Some(
            ".tokenade/semantic.{{bin,json}} (deleted legacy file — will rebuild on next index)"
                .into(),
        )
    }
}
