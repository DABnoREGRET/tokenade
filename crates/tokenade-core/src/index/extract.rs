//! **Pulling out of a file what the index records.**
//!
//! A fingerprint, declarations, and enough to turn a byte offset back into a
//! line number. Nothing here decides WHAT to index nor answers a question: it
//! is the passage from source text to table rows.

use super::*;

pub(super) fn compile(p: &str) -> &'static Regex {
    // Tiny cache of compiled regexes keyed by pattern string.
    static CACHE: OnceLock<std::sync::Mutex<BTreeMap<String, &'static Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()));
    let mut guard = cache.lock().unwrap();
    if let Some(r) = guard.get(p) {
        return r;
    }
    let leaked: &'static Regex = Box::leak(Box::new(Regex::new(p).unwrap()));
    guard.insert(p.to_string(), leaked);
    leaked
}

/// Single-file extraction result used by the parallel build. Free-standing
/// (not an `Index` method) so rayon can call it from worker threads without
/// needing a `&mut Index`.
pub(super) struct FilePartial {
    pub(super) rel: String,
    pub(super) symbols: Vec<Symbol>,
    pub(super) imports: Vec<String>,
    pub(super) mtime: u64,
    pub(super) calls: Vec<Call>,
}

/// Per-file change fingerprint stored in [`Index::mtimes`]. Folds the file's
/// high-resolution mtime (nanoseconds) AND its size, so a change is detected in
/// cases a bare seconds-mtime misses: two edits landing in the same wall-clock
/// second (the mtime is identical → size differs), a coarse-granularity
/// filesystem (Windows/FAT reporting only whole seconds → size differs), or an
/// mtime moving BACKWARD (checkout of an older revision → fingerprint differs,
/// whereas a `mtime <= prev` test would wrongly skip it). Metadata-only — no
/// content read — so incremental reindex stays proportional to changed files,
/// not the whole tree.
pub(crate) fn file_fingerprint(md: &std::fs::Metadata) -> u64 {
    let nanos = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64) // low 64 bits ≈ 584 years of ns — ample
        .unwrap_or(0);
    // FNV-style mix so equal-mtime / different-size edits fingerprint apart.
    nanos
        .wrapping_mul(0x0000_0100_0000_01b3)
        .wrapping_add(md.len())
}

pub(super) fn extract_file(root: &Path, path: &Path, ext: &str) -> Option<FilePartial> {
    let content = match fs::read_to_string(path) {
        Ok(c) if c.len() < 2_000_000 => c,
        _ => return None,
    };
    let rel = rel_path_string(path, root);
    let mtime = fs::metadata(path)
        .ok()
        .map(|m| file_fingerprint(&m))
        .unwrap_or(0);
    // A minified bundle under the size cap (a committed `vendor.min.js`) is a
    // parse of generated names: measured 2026-10-03, a 200 KB bundle made 3 000
    // of 3 001 symbols and topped `map`. Indexed EMPTY, not skipped — `None`
    // means "keep the last good symbols", which would freeze a source file
    // that a build minified in place.
    if crate::minified::detect(&content) {
        return Some(FilePartial {
            rel,
            symbols: Vec::new(),
            imports: Vec::new(),
            mtime,
            calls: Vec::new(),
        });
    }

    // Prefer tree-sitter when we ship a grammar for this language.
    // It catches strings/comments accurately + multi-line signatures
    // that the regex extractor stumbles on. Regex remains the fallback
    // for unsupported / new extensions.
    //
    // When a grammar lacks a useful "import" node — Ruby's `require`
    // is a method call, not a syntactic import — we backfill imports
    // with the regex extractor's pattern. That keeps `idx.imports`
    // populated even where the AST can't help.
    if let Some((mut ts_syms, mut ts_imports, mut ts_calls)) =
        crate::treesitter::extract_full(&content, ext)
    {
        for s in ts_syms.iter_mut() {
            s.file = rel.clone();
        }
        for c in ts_calls.iter_mut() {
            c.file = rel.clone();
        }
        if ts_imports.is_empty() {
            if let Some(spec) = lang_for(ext) {
                if let Some(imp) = spec.import {
                    for cap in compile(imp).captures_iter(&content) {
                        if let Some(m) = cap.get(1) {
                            ts_imports.push(m.as_str().to_string());
                        }
                    }
                }
            }
        }
        ts_imports.sort();
        ts_imports.dedup();
        return Some(FilePartial {
            rel,
            symbols: ts_syms,
            imports: ts_imports,
            mtime,
            calls: ts_calls,
        });
    }

    let spec = lang_for(ext)?;
    let line_starts = line_offsets(&content);
    let mut symbols = Vec::new();
    for (pat, kind) in spec.patterns {
        for cap in compile(pat).captures_iter(&content) {
            if let Some(m) = cap.get(1) {
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind: (*kind).to_string(),
                    file: rel.clone(),
                    line: line_at(&line_starts, m.start()),
                    decorators: Vec::new(),
                    bases: Vec::new(),
                    owner: String::new(),
                    // Regex extractor: no parse tree, so no hashes and no end
                    // line. Empty/zero is the honest value — a rename detector
                    // must not treat "unknown" as "identical".
                    structural_hash: String::new(),
                    self_hash: String::new(),
                    end_line: 0,
                });
            }
        }
    }
    let mut imports = Vec::new();
    if let Some(imp) = spec.import {
        for cap in compile(imp).captures_iter(&content) {
            if let Some(m) = cap.get(1) {
                imports.push(m.as_str().to_string());
            }
        }
        imports.sort();
        imports.dedup();
    }
    Some(FilePartial {
        rel,
        symbols,
        imports,
        mtime,
        calls: Vec::new(), // regex path doesn't extract calls
    })
}

/// Rows (1-based) carrying a DECLARATION or an import, for a language the
/// regex extractor knows, in source order and deduplicated.
///
/// The symbol index has understood 73 file extensions since it became a table;
/// the tree-sitter registry the skeleton drives from carries 16 grammars.
/// For the other 57 — Elixir, Lua, Haskell, Clojure, Dart, Julia, Nim, Zig,
/// OCaml, Erlang, Perl, R, PowerShell, shell, … — `skeleton` fell back to the
/// language-agnostic line heuristic while THIS table already held the exact
/// shape of a declaration in that language. Two doors, one file, two answers:
/// the same divergence once found between the index and the skeleton at the
/// level of a single `const`, here at the level of a whole language.
///
/// Line-anchored by construction (`(?m)^…`), so a signature wrapping several
/// physical lines yields only its first row. That is the honest limit of a
/// regex extractor and it is why this runs only where no grammar exists.
pub fn declaration_rows(ext: &str, text: &str) -> Option<Vec<usize>> {
    let spec = lang_for(ext)?;
    let starts = line_offsets(text);
    let mut rows = Vec::new();
    for (pat, _kind) in spec.patterns {
        for cap in compile(pat).captures_iter(text) {
            if let Some(m) = cap.get(1) {
                rows.push(line_at(&starts, m.start()));
            }
        }
    }
    if let Some(imp) = spec.import {
        for cap in compile(imp).captures_iter(text) {
            if let Some(m) = cap.get(1) {
                rows.push(line_at(&starts, m.start()));
            }
        }
    }
    rows.sort_unstable();
    rows.dedup();
    Some(rows)
}

pub(super) fn line_offsets(s: &str) -> Vec<usize> {
    let mut v = vec![0];
    for (i, c) in s.char_indices() {
        if c == '\n' {
            v.push(i + 1);
        }
    }
    v
}

pub(super) fn line_at(offsets: &[usize], byte: usize) -> usize {
    match offsets.binary_search(&byte) {
        Ok(i) => i + 1,
        Err(i) => i, // i is the count of starts <= byte
    }
}
