//! **Building the index** — from the repository to the posting lists.
//!
//! Two paths, and the second exists because the first is expensive: a full
//! rebuild re-reads EVERY file to extract comments and docstrings.
//! `rebuild_incremental` starts again from a snapshot and only re-reads what
//! has moved.
//!
//! Both share the same bounds — [`MAX_TOKENS_PER_DOC`] per file,
//! [`MAX_TOTAL_POSTINGS`] over the whole index. An uncapped index does not
//! fail: it eats the machine's memory, which is worse.

use super::*;

impl SemanticIndex {
    /// Build a BM25 index from a symbol [`Index`] plus the raw file contents
    /// under `root`. We re-read each file only to extract its docstrings and
    /// comments; symbol/import data is taken straight from `idx`.
    ///
    /// Bounded by [`MAX_TOKENS_PER_DOC`] per file and [`MAX_TOTAL_POSTINGS`]
    /// over the whole index. Files exceeding the per-doc cap are truncated
    /// to their head; the global cap stops adding postings when reached.
    pub fn build_from_index(idx: &Index, root: &Path) -> SemanticIndex {
        let mut by_file: HashMap<String, Vec<String>> = HashMap::new();
        // Seed each known file with its path components and declared symbols.
        for sym in &idx.symbols {
            by_file
                .entry(sym.file.clone())
                .or_default()
                .push(sym.name.clone());
        }
        // Make sure every indexed file is represented even if it had zero
        // symbols (e.g. a markdown README we still want to find).
        for f in idx.imports.keys() {
            by_file.entry(f.clone()).or_default();
        }

        let mut sem = SemanticIndex::default();
        let mut total_len: u64 = 0;
        let mut total_postings: usize = 0;
        let mut truncated_files: usize = 0;
        // Visibility on builds that take a noticeable moment (~100 ms+).
        // by_file is a `HashMap` here so its `.len()` is exact and cheap.
        let by_file_count = by_file.len();
        let started_sem = std::time::Instant::now();
        let announce_sem = by_file_count >= 50;
        if announce_sem {
            crate::progress!(
                "[tokenade] building semantic search index over {by_file_count} file(s)…"
            );
        }
        // Deterministic order, and it is not cosmetic: the loop below STOPS at
        // `MAX_TOTAL_POSTINGS`. Walking a `HashMap` means the cap cuts a
        // different subset of files on every rebuild, so on a repository big
        // enough to reach it the index — and therefore what `semantic` can
        // find at all — changed from one run to the next. Sorted, the cut is
        // reproducible and explainable to whoever asks why their file is not
        // searchable.
        let mut by_file: Vec<(String, Vec<String>)> = by_file.into_iter().collect();
        by_file.sort_by(|a, b| a.0.cmp(&b.0));
        for (file, symbols) in by_file {
            if total_postings >= MAX_TOTAL_POSTINGS {
                eprintln!(
                    "[tokenade] semantic index posting cap reached ({MAX_TOTAL_POSTINGS}); \
                     remaining files unindexed for search"
                );
                break;
            }
            let path = root.join(&file);
            let comments = read_comments(&path).unwrap_or_default();

            let mut text = String::with_capacity(symbols.len() * 8 + comments.len());
            for c in path_components(&file) {
                text.push_str(&c);
                text.push(' ');
            }
            for s in &symbols {
                text.push_str(s);
                text.push(' ');
                text.push_str(s);
                text.push(' ');
                text.push_str(s);
                text.push(' ');
            }
            text.push_str(&comments);

            let mut tokens = tokenize(&text);
            if tokens.is_empty() {
                continue;
            }
            if tokens.len() > MAX_TOKENS_PER_DOC {
                tokens.truncate(MAX_TOKENS_PER_DOC);
                truncated_files += 1;
            }
            let doc_id = sem.docs.len() as u32;
            let mut tf: HashMap<String, u32> = HashMap::new();
            for t in &tokens {
                *tf.entry(t.clone()).or_insert(0) += 1;
            }
            sem.docs.push(Doc {
                file: file.clone(),
                len: tokens.len() as u32,
                snippet: build_snippet(&file, &symbols, &comments),
            });
            // Record the file's current mtime so a future
            // `rebuild_incremental` can skip it if untouched.
            if let Some(&mt) = idx.mtimes.get(&file) {
                sem.mtimes.insert(file.clone(), mt);
            }
            total_len += tokens.len() as u64;
            for (term, n) in tf {
                sem.inv
                    .entry(term)
                    .or_default()
                    .push(Posting { doc: doc_id, tf: n });
                total_postings += 1;
            }
        }
        if truncated_files > 0 {
            eprintln!(
                "[tokenade] semantic index: {truncated_files} file(s) truncated at \
                 {MAX_TOKENS_PER_DOC} tokens — symbol/path data still indexed"
            );
        }
        sem.avgdl = if sem.docs.is_empty() {
            0.0
        } else {
            total_len as f64 / sem.docs.len() as f64
        };
        if announce_sem {
            crate::progress!(
                "[tokenade] semantic index built ({} doc(s), {} postings) in {}ms.",
                sem.docs.len(),
                total_postings,
                started_sem.elapsed().as_millis()
            );
        }
        sem
    }

    /// Incrementally rebuild from a previous snapshot. Files whose mtime in
    /// `idx.mtimes` matches `prev.mtimes` keep their docstring tokens; only
    /// changed / new files are re-read and re-tokenised. Files dropped from
    /// the symbol index are removed.
    ///
    /// On a 100 k-file monorepo where 1 file changed, this turns a 30-s
    /// full rebuild into a <100 ms diff. Falls back to a full rebuild when
    /// `prev` has no mtime data (e.g. loaded from a pre-v6 binary).
    pub fn rebuild_incremental(prev: &SemanticIndex, idx: &Index, root: &Path) -> SemanticIndex {
        if prev.mtimes.is_empty() || prev.docs.is_empty() {
            return Self::build_from_index(idx, root);
        }
        // Files that need a fresh tokenisation: new files OR files whose
        // mtime moved forward.
        let mut to_rebuild: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Files that no longer exist in the source tree: drop them from
        // the new index entirely.
        let live: std::collections::HashSet<&String> = idx.mtimes.keys().collect();
        for (file, &mt) in &idx.mtimes {
            match prev.mtimes.get(file) {
                Some(&prev_mt) if prev_mt == mt => {} // unchanged — reuse
                _ => {
                    to_rebuild.insert(file.clone());
                }
            }
        }
        // Files in the previous index whose path is no longer present.
        let dropped: std::collections::HashSet<String> = prev
            .mtimes
            .keys()
            .filter(|f| !live.contains(*f))
            .cloned()
            .collect();
        // Optimisation skip: nothing changed → return a clone.
        if to_rebuild.is_empty() && dropped.is_empty() {
            return prev.clone();
        }

        // Step 1: start from prev, drop files that are gone or changing.
        let mut sem = prev.clone();
        let drop_set: std::collections::HashSet<String> = dropped
            .iter()
            .cloned()
            .chain(to_rebuild.iter().cloned())
            .collect();
        if !drop_set.is_empty() {
            // Map old doc IDs to keep-or-drop.
            let mut keep_doc: Vec<Option<u32>> = vec![None; sem.docs.len()];
            let mut new_docs: Vec<Doc> = Vec::with_capacity(sem.docs.len());
            for (old_id, d) in sem.docs.iter().enumerate() {
                if !drop_set.contains(&d.file) {
                    keep_doc[old_id] = Some(new_docs.len() as u32);
                    new_docs.push(d.clone());
                }
            }
            sem.docs = new_docs;
            sem.mtimes.retain(|f, _| !drop_set.contains(f));
            // Renumber postings; drop postings whose doc was removed.
            let mut new_inv: HashMap<String, Vec<Posting>> = HashMap::new();
            for (term, postings) in sem.inv.drain() {
                let mut kept = Vec::with_capacity(postings.len());
                for p in postings {
                    if let Some(Some(new_id)) = keep_doc.get(p.doc as usize) {
                        kept.push(Posting {
                            doc: *new_id,
                            tf: p.tf,
                        });
                    }
                }
                if !kept.is_empty() {
                    new_inv.insert(term, kept);
                }
            }
            sem.inv = new_inv;
        }

        // Step 2: tokenise the to_rebuild files and append to the index.
        // We re-use the same logic as `build_from_index` but constrained
        // to the smaller set.
        let mut by_file: HashMap<String, Vec<String>> = HashMap::new();
        for sym in &idx.symbols {
            if to_rebuild.contains(&sym.file) {
                by_file
                    .entry(sym.file.clone())
                    .or_default()
                    .push(sym.name.clone());
            }
        }
        for f in idx.imports.keys() {
            if to_rebuild.contains(f) {
                by_file.entry(f.clone()).or_default();
            }
        }
        let mut total_len: u64 = sem.docs.iter().map(|d| d.len as u64).sum();
        let mut total_postings: usize = sem.inv.values().map(|v| v.len()).sum();
        // Same cap, same rule as the full build above.
        let mut by_file: Vec<(String, Vec<String>)> = by_file.into_iter().collect();
        by_file.sort_by(|a, b| a.0.cmp(&b.0));
        for (file, symbols) in by_file {
            if total_postings >= MAX_TOTAL_POSTINGS {
                break;
            }
            let path = root.join(&file);
            let comments = read_comments(&path).unwrap_or_default();
            let mut text = String::with_capacity(symbols.len() * 8 + comments.len());
            for c in path_components(&file) {
                text.push_str(&c);
                text.push(' ');
            }
            for s in &symbols {
                text.push_str(s);
                text.push(' ');
                text.push_str(s);
                text.push(' ');
                text.push_str(s);
                text.push(' ');
            }
            text.push_str(&comments);
            let mut tokens = tokenize(&text);
            if tokens.is_empty() {
                continue;
            }
            if tokens.len() > MAX_TOKENS_PER_DOC {
                tokens.truncate(MAX_TOKENS_PER_DOC);
            }
            let doc_id = sem.docs.len() as u32;
            let mut tf: HashMap<String, u32> = HashMap::new();
            for t in &tokens {
                *tf.entry(t.clone()).or_insert(0) += 1;
            }
            sem.docs.push(Doc {
                file: file.clone(),
                len: tokens.len() as u32,
                snippet: build_snippet(&file, &symbols, &comments),
            });
            if let Some(&mt) = idx.mtimes.get(&file) {
                sem.mtimes.insert(file.clone(), mt);
            }
            total_len += tokens.len() as u64;
            for (term, n) in tf {
                sem.inv
                    .entry(term)
                    .or_default()
                    .push(Posting { doc: doc_id, tf: n });
                total_postings += 1;
            }
        }
        sem.avgdl = if sem.docs.is_empty() {
            0.0
        } else {
            total_len as f64 / sem.docs.len() as f64
        };
        sem
    }
}
