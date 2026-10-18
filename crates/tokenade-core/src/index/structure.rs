//! **The map: what the repo contains, seen from above.**
//!
//! Not an answer to a precise question, but the view an agent reads first to
//! know where to look. The per-directory rendering is here because the map is
//! the only thing that consumes it.

use super::*;

/// How many directories a text structure map lists before it starts ranking.
/// Below this the whole tree is shown, byte-for-byte as before.
pub(super) const MAP_MAX_DIRS: usize = 80;

/// Render the `by directory:` block, bounded.
///
/// The block was unbounded, and it is the bulk of the output: measured on a
/// 7 891-file repository, 1 113 of 1 147 lines were directories and 756 of
/// those held a SINGLE file — a "cheap overview" that cost more than reading
/// several files outright. Keeping the densest directories keeps the shape
/// (the top 80 carry ~73% of the symbols in ~7% of the lines) and the tail
/// that goes is the part that was never orienting anyone.
///
/// Selected by CODE symbol count — a directory holding one function and a
/// dozen tests around it is not a dozen-symbol directory — then printed BY
/// PATH so the tree still reads like a tree. Ties break on path so two processes agree. `--json` is untouched:
/// a CI consumer asked for the whole list and gets it.
pub(super) fn render_by_dir(by_dir: &BTreeMap<String, (usize, usize, usize)>, out: &mut String) {
    // `(+N test)` rather than a bare total: a directory of one function and a
    // dozen tests around it is not a dozen-symbol directory, and this list is
    // what an agent reads to decide where the code is.
    let line = |d: &str, files: &usize, syms: &usize, tests: &usize| -> String {
        let t = if *tests > 0 {
            format!(" (+{tests} test)")
        } else {
            String::new()
        };
        format!("  {d}/   {files} files, {syms} symbols{t}\n")
    };
    out.push_str("by directory:\n");
    if by_dir.len() <= MAP_MAX_DIRS {
        for (d, (files, syms, tests)) in by_dir {
            out.push_str(&line(d, files, syms, tests));
        }
        return;
    }
    let mut ranked: Vec<(&String, &(usize, usize, usize))> = by_dir.iter().collect();
    ranked.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then_with(|| a.0.cmp(b.0)));
    let dropped = ranked.split_off(MAP_MAX_DIRS.min(ranked.len()));
    // Ranking by code makes a repository's test tree rank last — on this one
    // it removed `tests/e2e/` and `tests/` from the map outright, and a
    // suite of several hundred declarations is orienting even though nothing
    // depends on it. Named rather than shown: two lines, not eighty.
    let mut test_trees: Vec<(&String, usize)> = dropped
        .iter()
        .filter(|(_, (_, code, tests))| *tests > 0 && *code == 0)
        .map(|(d, (_, _, tests))| (*d, *tests))
        .collect();
    test_trees.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    test_trees.truncate(2);
    let mut kept: Vec<(&String, &(usize, usize, usize))> = ranked;
    kept.sort_by(|a, b| a.0.cmp(b.0));
    for (d, (files, syms, tests)) in &kept {
        out.push_str(&line(d, files, syms, tests));
    }
    let _ = writeln!(
        out,
        "  [{} of {} directories shown — the densest in CODE, tests not counted toward the ranking{}; run map from a subdirectory for its detail, or --json for the full list]",
        MAP_MAX_DIRS,
        by_dir.len(),
        if test_trees.is_empty() {
            String::new()
        } else {
            format!(
                ". Test trees left out: {}",
                test_trees
                    .iter()
                    .map(|(d, n)| format!("{d}/ ({n} test)"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );
}

impl Index {
    /// Same as [`structure_map`] but reranks the top-files list to surface
    /// framework-canonical paths first (Next.js `pages/` / `app/`, Django
    /// `views.py` / `urls.py`, etc.). When `fw` is `Unknown` this is
    /// identical to `structure_map`.
    pub fn structure_map_with_framework(&self, fw: crate::framework::Framework) -> String {
        self.structure_map_under(fw, None)
    }

    /// Does `file` live under `prefix`? A prefix names a directory or a
    /// single file, so `src/net` must not swallow `src/network.rs`: the
    /// match is the path itself or the path followed by a separator.
    pub fn path_is_under(file: &str, prefix: &str) -> bool {
        let p = prefix.trim_start_matches("./").trim_end_matches('/');
        if p.is_empty() {
            return true;
        }
        file == p || file.starts_with(&format!("{p}/"))
    }

    /// The structure map, optionally narrowed to one subtree.
    ///
    /// `prefix: None` is the whole workspace, byte-for-byte what
    /// [`structure_map_with_framework`] has always printed. `Some(path)`
    /// answers "show me a structure map of THIS directory", which is what
    /// `tokenade map <path>` was refused for.
    ///
    /// The counts in the header are the counts of the NARROWED set. A map
    /// that says "1 044 files" above a listing of one directory is the same
    /// lie as a capped list that says nothing: the header is the only figure
    /// most readers take away.
    pub fn structure_map_under(
        &self,
        fw: crate::framework::Framework,
        prefix: Option<&str>,
    ) -> String {
        let keep_file = |f: &str| match prefix {
            None => true,
            Some(p) => Self::path_is_under(f, p),
        };
        // Rank by CODE, count tests apart. `map` is the orientation command,
        // and 44% of this workspace's indexed symbols are test declarations —
        // enough to decide which 80 of 108 directories survive the cut.
        // Measured before this: eight `*_dispatcher/` directories holding ONE
        // code symbol each were kept on the strength of 8 to 12 tests, while
        // directories with real code were dropped, and three of the ten "top
        // files" were ranked by their test module. The comment on the
        // canonical-path boost below already knew the shape of this — it
        // exists so "a single-symbol canonical beats a 30-symbol test helper
        // file" — but only for framework entry points.
        let mut spans_by_file: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
        for s in &self.symbols {
            if s.kind == "module" && (s.name == "tests" || s.name == "test") && s.end_line > s.line
            {
                spans_by_file
                    .entry(s.file.as_str())
                    .or_default()
                    .push((s.line, s.end_line));
            }
        }
        const NO_SPANS: &[(usize, usize)] = &[];
        // (files, code symbols, test symbols)
        let mut by_dir: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
        let mut by_file: BTreeMap<&str, usize> = BTreeMap::new();
        let mut tests_by_file: BTreeMap<&str, usize> = BTreeMap::new();
        for s in &self.symbols {
            if !keep_file(&s.file) {
                continue;
            }
            let spans = spans_by_file
                .get(s.file.as_str())
                .map(|v| v.as_slice())
                .unwrap_or(NO_SPANS);
            let e = by_dir
                .entry(parent_dir(&s.file).to_string())
                .or_insert((0, 0, 0));
            if symbol_is_test(s, spans) {
                *tests_by_file.entry(&s.file).or_insert(0) += 1;
                e.2 += 1;
            } else {
                *by_file.entry(&s.file).or_insert(0) += 1;
                e.1 += 1;
            }
            // A file that is ALL tests must still appear as a file.
            by_file.entry(&s.file).or_insert(0);
        }
        for file in by_file.keys() {
            if let Some(e) = by_dir.get_mut(parent_dir(file)) {
                e.0 += 1;
            }
        }

        // An empty narrowing is a question with an answer, not an empty map.
        // Printing `0 files, 0 symbols indexed` under a `by directory:`
        // heading reads as "this subtree holds nothing", which is
        // indistinguishable from "you spelled the path differently than the
        // index did".
        if prefix.is_some() && by_file.is_empty() {
            return format!(
                "no indexed file under '{}' ({} files indexed in this workspace). \
                 Paths are matched from your current directory; one spelled from the \
                 workspace root is accepted too.",
                prefix.unwrap_or_default(),
                self.files_indexed
            );
        }

        let (files, symbols) = match prefix {
            None => (self.files_indexed, self.symbols.len()),
            Some(_) => (
                by_file.len(),
                by_dir.values().map(|(_, s, t)| *s + *t).sum(),
            ),
        };
        let mut out = format!("{files} files, {symbols} symbols indexed");
        match prefix {
            Some(p) => {
                let _ = writeln!(out, " under {p}");
            }
            None => out.push('\n'),
        }
        if fw != crate::framework::Framework::Unknown {
            let _ = writeln!(out, "framework: {}", fw.as_str());
        }
        render_by_dir(&by_dir, &mut out);

        // Boost framework-canonical files so they float into the top regardless
        // of raw symbol count — entry-point files (e.g. Next `app/page.tsx`)
        // often have few symbols but matter most to an agent. We give a
        // *floor* (additive) plus a multiplier so even a single-symbol
        // canonical beats a 30-symbol test helper file.
        let canon = fw.canonical_paths();
        let mut scored: Vec<(&&str, f64)> = by_file
            .iter()
            .map(|(f, n)| {
                let is_canon = canon.iter().any(|p| f.contains(p));
                let score = if is_canon {
                    (*n as f64) * 5.0 + 50.0
                } else {
                    *n as f64
                };
                (f, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let keep = if fw == crate::framework::Framework::Unknown {
            10
        } else {
            30
        };
        let total_files = scored.len();
        scored.truncate(keep);
        if !scored.is_empty() {
            // Say how many were left out, like the directory block above and
            // like `impact`/`query`. A ranked list that stops without a word
            // reads as the whole set — and "top files" is exactly the phrase a
            // reader takes for "these ARE the files that matter".
            if total_files > keep {
                let _ = writeln!(out, "top files ({keep} of {total_files}, densest first):");
            } else {
                out.push_str("top files:\n");
            }
            for (f, score) in scored {
                let star = if canon.iter().any(|p| f.contains(p)) {
                    " ★"
                } else {
                    ""
                };
                let syms = by_file.get(*f).copied().unwrap_or(0);
                let tsyms = tests_by_file.get(*f).copied().unwrap_or(0);
                let t = if tsyms > 0 {
                    format!(" +{tsyms} test")
                } else {
                    String::new()
                };
                let _ = writeln!(out, "  {f}  ({syms} symbols{t}){star}");
                let _ = score; // silence unused
            }
        }
        out.trim_end().to_string()
    }

    /// A compact one-screen map of the whole codebase.
    ///
    /// Grouped by top-level directory (so 1000-file repos still fit on one
    /// screen), plus the 10 files with the most symbols — the "where the
    /// interesting code lives" view an agent actually needs.
    ///
    /// One renderer, not two. This body was a second copy of
    /// [`structure_map_under`] with the framework boost removed, which is
    /// what `Framework::Unknown` already does — verified byte-for-byte on
    /// this repository before the two were merged, and asserted below so
    /// they cannot drift apart again.
    pub fn structure_map(&self) -> String {
        self.structure_map_under(crate::framework::Framework::Unknown, None)
    }
}
