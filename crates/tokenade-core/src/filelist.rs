//! `find` / `tree` / `ls -laR` directory listing compaction.
//!
//! When an agent does `find . -type f` (or `git ls-files`, or `tree -L 5`)
//! on a real repo, output is one path per line. On a moderate project that's
//! 1 000 – 50 000 lines:
//!
//! ```text
//! ./Cargo.toml
//! ./crates/tokenade-core/src/lib.rs
//! ./crates/tokenade-core/src/auto.rs
//! ./crates/tokenade-core/src/openapi.rs
//! …5 000 more paths…
//! ```
//!
//! What the agent reading this actually wants:
//!
//!   - the **shape** of the tree: how many files in each top-level dir
//!   - the **language/extension mix**: `.rs: 412, .toml: 8, .md: 23, …`
//!   - the **total file count** and number of top-level directories
//!
//! Compact output is a small fraction of the original. The compactor
//! recognises three shapes:
//!
//!   * `find` output — most lines look like paths starting with `./` or `/`
//!   * `tree` output — contains tree-drawing characters (`├──`, `└──`, `│  `)
//!   * `ls -laR` output — contains `total N` lines + permission strings

use std::collections::BTreeMap;
use std::fmt::Write;

pub const TOP_DIRS_CAP: usize = 50;
pub const EXTENSIONS_CAP: usize = 30;

/// Minimum raw byte count below which we PASS THROUGH the original
/// listing untouched. Compaction loses concrete path strings —
/// replacing them with a summary only pays off when the raw output
/// is large enough that the summary saves real tokens. Below ~4 KB
/// (~1 k tokens), the raw paths are more useful than the summary.
///
/// Tuned in response to a user report on 2026-05-29: `find -name .git
/// -prune | sed | sort` produced 524 B of repo paths; auto-compaction
/// replaced the list with `[tokenade:filelist] find, 21 file(s)
/// across 4 dirs`, destroying the actual paths the user wanted.
pub const MIN_BYTES_TO_FOLD: usize = 4_096;

/// Minimum file count below which we PASS THROUGH. Same reasoning:
/// a 21-path list is short enough to read literally; only larger
/// listings benefit from the directory/extension rollup.
pub const MIN_FILES_TO_FOLD: usize = 60;

mod detect;
pub use detect::detect;
pub(crate) use detect::is_windows_path;
#[cfg(test)]
use detect::{carries_a_version, looks_like_dependency_tree};
use detect::{detect_shape, listing_line_is_path, Shape};

/// Strip the leading `./` or `/` so we always group on the same top-level
/// names regardless of which form `find` produced.
fn normalise_path(s: &str) -> &str {
    s.strip_prefix("./")
        .unwrap_or_else(|| s.strip_prefix('/').unwrap_or(s))
}

/// Number of leading `/`-separated segments shared by EVERY path.
///
/// One declarer, three callers: this listing's grouping, `grep`'s
/// path-prefix strip, and the Next.js route table's group label. All
/// three used to compute it themselves, and the three spellings had
/// already drifted — one byte-wise then cut back to the last `/`, one
/// segment-wise, one over arbitrary lines rather than paths. Segments,
/// never bytes: a byte-wise prefix of `crates/a` and `crates/b` is
/// **The common path prefix, said ONCE.**
///
/// Returns the header to write and the number of bytes to strip from the head
/// of each line, or `None` when it does not pay. The whole decision lives here:
/// the prefix is measured in SEGMENTS and never in bytes, it must be at least
/// ten characters, it must save more than its own header, and the wording
/// changes when every line names the same file — there is then no directory to
/// join.
///
/// Written once because it was only ever one thing: `cmd/linux/grep.rs` carried
/// this decision privately, and the twenty listing commands that go through
/// `git_listing::fold_paths` — `find`, `fd`, `rg --files`, `git ls-files`,
/// `tar tzf`, `rpm -ql`, `dpkg -L`, `grep -l` — shipped the prefix on every
/// line. MEASURED 2026-09-08 on the corpus: `/usr/share/doc` appears
/// **236 times in 6 captures, 11 138 bytes**, and `find /usr/share/doc` is the
/// largest capture still shipped in the corpus.
pub(crate) fn common_prefix_header(paths: &[&str]) -> Option<(String, usize)> {
    if paths.len() < 3 {
        return None;
    }
    let sep = separator_of(paths);
    let first_segments: Vec<&str> = paths[0].split(sep).collect();
    let common = common_path_segments(paths);
    if common == 0 {
        return None;
    }
    let prefix = first_segments[..common].join(&sep.to_string());
    // Too short, the header costs more than it gives back in readability.
    if prefix.len() < 10 {
        return None;
    }
    // No profitability gate here: it would be dead. With the two bounds
    // above — at least three paths, a prefix of at least ten characters —
    // the fold saves `3 × (p + 1)` against a header of `p + 18`, and
    // `3p + 3 > p + 18` as soon as `p ≥ 8`. A mutation that opened it SURVIVED,
    // and it was a no-op, not a suite gap. The property that matters is
    // pinned by `the_smallest_case_still_pays`, which goes red if either
    // bound goes down.
    // Every line on the SAME file: the prefix is that file, and announcing
    // it as a directory gives `a/b/hcl.rs/128`, a path that does not
    // open.
    let header = if paths.iter().all(|p| *p == paths[0]) {
        format!("[all matches in {prefix}]")
    } else {
        format!("[paths from {prefix}{sep}]")
    };
    Some((header, prefix.len() + 1))
}

/// `\\` when every path is a Windows one (`C:\\x\\y`, no `/` anywhere),
/// else `/`. A real `Select-String` over one file (2026-10-05) repeated its
/// 40-byte `C:\\…\\services` prefix on every one of 200 hits because the
/// prefix was looked for between `/`s only.
pub(crate) fn separator_of(paths: &[&str]) -> char {
    if !paths.is_empty() && paths.iter().all(|p| p.contains('\\') && !p.contains('/')) {
        '\\'
    } else {
        '/'
    }
}

/// `crates/`, but of `crates/foo` and `crates/font` it is `crates/fo`,
/// which is not a path component.
///
/// The count is the RAW shared prefix — the final segment of a path is
/// a filename and this function does not know that. A caller wanting a
/// shared DIRECTORY caps the answer itself (`n.min(segments - 1)`),
/// because whether the last segment is a file is the caller's question,
/// not this one's.
/// Writes a listing: the common prefix removed, and each directory said once
/// with its files below it.
///
/// `path_of` extracts the PATH part of a line: identity for a file listing,
/// and for `grep -n` the part before `:<number>:` — otherwise the last `/` of
/// the matched text would pass for a directory separator. The two renderings
/// used to be written separately; the `grep` one only had the prefix
/// removal.
pub(crate) fn push_grouped(
    out: &mut String,
    lines: &[&str],
    strip: usize,
    path_of: &dyn Fn(&str) -> Option<&str>,
) {
    // Only a line that names a path carries the prefix; a wrapped
    // continuation is kept whole.
    let kept: Vec<&str> = lines
        .iter()
        .map(|l| {
            if l.len() > strip && path_of(l).is_some() {
                &l[strip..]
            } else {
                *l
            }
        })
        .collect();
    let mut i = 0;
    while i < kept.len() {
        // **The RUN is computed once, and we always advance by its
        // length.** The first version restarted one step further when the
        // group was refused, and rescanned the same run from every line:
        // MEASURED 2026-09-09, forty thousand lines of `grep -rn` under a
        // one-character directory — a perfectly ordinary case — still had
        // not returned after two minutes. The refusal is MONOTONIC: a
        // shorter run under the same directory saves less, so if this one
        // does not pay, none of its tails will.
        let (dir, n) = homogeneous_run(&kept[i..], path_of);
        if let Some(dir) = dir.filter(|d| grouping_pays(d, &kept[i..i + n])) {
            let _ = writeln!(out, "{dir}/");
            for l in &kept[i..i + n] {
                let _ = writeln!(out, "  {}", &l[dir.len() + 1..]);
            }
            i += n;
            continue;
        }
        // `find` lists the directory ITSELF before its contents, so the line
        // `faad2` precedes the `faad2/` group. The group header already says it.
        // The look-ahead costs only one run, the one the next iteration
        // consumes entirely.
        if n == 1 {
            let (next_dir, m) = homogeneous_run(kept.get(i + 1..).unwrap_or(&[]), path_of);
            if let Some(d) = next_dir.filter(|d| grouping_pays(d, &kept[i + 1..i + 1 + m])) {
                if kept[i] == d {
                    i += 1;
                    continue;
                }
            }
        }
        for l in &kept[i..i + n] {
            out.push_str(l);
            out.push('\n');
        }
        i += n;
    }
}

/// The run of CONSECUTIVE lines under one same directory, and its length.
///
/// `None` in first position when the line has no directory: the run is then
/// 1, and the caller advances by that much.
fn homogeneous_run<'a>(
    lines: &[&'a str],
    path_of: &dyn Fn(&str) -> Option<&str>,
) -> (Option<&'a str>, usize) {
    let Some(first) = lines.first() else {
        return (None, 0);
    };
    let Some(dir_len) = path_of(first)
        .and_then(|p| p.rsplit_once('/'))
        .map(|(d, _)| d.len())
    else {
        return (None, 1);
    };
    let dir = &first[..dir_len];
    if dir.is_empty() {
        return (None, 1);
    }
    let n = lines
        .iter()
        .take_while(|l| path_of(l).and_then(|p| p.rsplit_once('/')).map(|(d, _)| d) == Some(dir))
        .count();
    (Some(dir), n)
}

/// Does grouping pay? The header and the two spaces are not free.
///
/// The gate is the repo's, `fold_gate::pays`, and its 10% margin is not a
/// comfort setting: a fold that saves 2% changes the shape of the output for
/// nothing, and a changed shape is a cost bytes do not measure.
/// This was the FOURTH private copy of this question.
fn grouping_pays(dir: &str, lines: &[&str]) -> bool {
    if lines.len() < 3 {
        return false;
    }
    let before: usize = lines.iter().map(|l| l.len() + 1).sum();
    let after: usize = dir.len() + 2 + lines.iter().map(|l| l.len() - dir.len() + 2).sum::<usize>();
    crate::fold_gate::pays(before, after, 0)
}

/// **A LIST of paths on a single line: the root, said once.**
///
/// `lowerdir=/a/b/l/X:/a/b/l/Y:/a/b/l/Z` becomes `lowerdir=/a/b/l/{X:Y:Z}`.
/// Nothing disappears — every name is still there, in order — it is the
/// repeated root that goes, and it carries no information after the first time.
///
/// [`common_prefix_header`] answers the same question for paths on separate
/// LINES; both go through [`common_path_segments`], which counts SEGMENTS and
/// not bytes (otherwise `/a/foo` and `/a/font` would share `/a/fo`). A general
/// mechanism with a single caller does not earn its keep; this one has a
/// second.
///
/// MEASURED 2026-09-09 on `mount`: four Docker `overlay` lines carry 8 to 12
/// layers under the same root `/var/lib/docker/overlay2/l`, i.e.
/// **918 bytes of repeated root out of the 3 492 those lines weigh**.
pub fn collapse_path_list(value: &str) -> Option<String> {
    let parts: Vec<&str> = value.split(':').collect();
    // Below three, the root costs almost as much as it saves, and a value
    // containing a `:` is not necessarily a list (`c:\` on Windows, a
    // `host:port`).
    if parts.len() < 3 || !parts.iter().all(|p| p.starts_with('/') && p.len() > 1) {
        return None;
    }
    let common = common_path_segments(&parts);
    if common == 0 {
        return None;
    }
    let root: String = parts[0]
        .split('/')
        .take(common)
        .collect::<Vec<_>>()
        .join("/");
    if root.len() < 10 {
        return None;
    }
    let tails: Vec<&str> = parts
        .iter()
        .map(|p| p[root.len()..].trim_start_matches('/'))
        .collect();
    let folded = format!("{root}/{{{}}}", tails.join(":"));
    // Same gate as the rest of the repo: at least ten percent, otherwise we
    // change the shape of a line for nothing.
    crate::fold_gate::pays(value.len(), folded.len(), 0).then_some(folded)
}

pub(crate) fn common_path_segments(paths: &[&str]) -> usize {
    let Some(first) = paths.first() else {
        return 0;
    };
    let sep = separator_of(paths);
    let first: Vec<&str> = first.split(sep).collect();
    let mut common = first.len();
    for p in &paths[1..] {
        let mut matched = 0;
        for (i, seg) in p.split(sep).enumerate() {
            if i >= common || first[i] != seg {
                break;
            }
            matched = i + 1;
        }
        common = matched;
        if common == 0 {
            return 0;
        }
    }
    common
}

/// For a normalised path like `crates/tokenade-core/src/lib.rs`, return the
/// top-level dir (`crates`) and the extension (`.rs`). The top-level dir
/// is `"(root)"` when the path has no `/`.
fn split_top_and_ext(path: &str) -> (String, Option<String>) {
    let top = match path.find('/') {
        Some(i) => path[..i].to_string(),
        None => "(root)".to_string(),
    };
    let ext = path.rfind('.').and_then(|i| {
        // Make sure the `.` is in the basename, not a path component.
        let after = &path[i..];
        if after.contains('/') || i == 0 {
            None
        } else {
            // Hidden files like `.gitignore` count as "no extension".
            let last_slash = path.rfind('/').map(|s| s + 1).unwrap_or(0);
            if last_slash == i {
                None
            } else {
                Some(after.to_string())
            }
        }
    });
    (top, ext)
}

/// `split_top_and_ext`, keyed BELOW a shared root.
///
/// `skip` segments are dropped before the grouping key is taken. The
/// extension is unaffected: it comes from the basename, which no amount
/// of skipping reaches.
fn split_top_and_ext_below(path: &str, skip: usize) -> (String, Option<String>) {
    let (_, ext) = split_top_and_ext(path);
    let rest: Vec<&str> = path.split('/').skip(skip).collect();
    let top = match rest.len() {
        0 | 1 => "(root)".to_string(),
        _ => rest[0].to_string(),
    };
    (top, ext)
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    // A Windows listing: every rule below reads `/`, and a Windows path never
    // holds one — so the listing is folded with `\\` read as `/`, and the
    // body handed back with its own separator (the banner keeps its text, and
    // no sentence this module writes holds a `/`: it would come back as `\\`).
    let paths = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if paths
        .clone()
        .take(60)
        .filter(|l| detect::is_windows_path(l))
        .count()
        >= 10
        && !text.contains('/')
    {
        // `C:\\x\\y` is folded as `/C:/x/y` — the drive is the first segment,
        // so every rule rooted at `/` applies — and given back as typed.
        let unix: String = text
            .lines()
            .map(|l| {
                let t = l.trim();
                if detect::is_windows_path(t) {
                    let p = t.replace('\\', "/");
                    if p.starts_with("./") {
                        p
                    } else {
                        format!("/{p}")
                    }
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let drive = crate::regexes::re(r"/([A-Za-z]):/");
        return compact_paths(&unix)
            .lines()
            .map(|l| {
                if l.starts_with("[tokenade") {
                    l.to_string()
                } else {
                    drive.replace_all(l, "${1}:/").replace('/', "\\")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    compact_paths(text)
}

fn compact_paths(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }
    let shape = detect_shape(text);

    // Pre-flight pass: count path-shaped entries. Short listings
    // (<MIN_BYTES_TO_FOLD raw OR <MIN_FILES_TO_FOLD entries) are
    // returned untouched — the directory/extension rollup destroys
    // the actual paths, and for short outputs the paths are more
    // useful than the summary.
    if text.len() < MIN_BYTES_TO_FOLD {
        let mut quick_count = 0usize;
        for raw in text.lines() {
            let t = raw.trim();
            if t.is_empty() {
                continue;
            }
            let counts_as_file = match shape {
                Shape::Find => listing_line_is_path(t),
                Shape::Tree => t.contains("── "),
                Shape::LsLaR => t.starts_with("-rw") || t.starts_with("-r-"),
            };
            if counts_as_file {
                quick_count += 1;
                if quick_count >= MIN_FILES_TO_FOLD {
                    break;
                }
            }
        }
        if quick_count < MIN_FILES_TO_FOLD {
            return text.trim_end().to_string();
        }
    }

    let mut by_dir: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_dir_ext: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut by_ext: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_files = 0usize;
    let mut unmatched: Vec<String> = Vec::new();
    // Collected first, grouped second. The grouping key is the first path
    // segment BELOW the root every entry shares, and that root is not known
    // until the last line is read.
    let mut paths: Vec<&str> = Vec::new();
    // The paths as WRITTEN, for the rendering that lists them: `normalise_path`
    // strips the leading `/`, fine for counting, wrong for shipping.
    let mut written: Vec<&str> = Vec::new();
    // `normalise_path` drops the leading `/`, so the shared root would be
    // rendered as a relative path it is not. Remembered here, restored in
    // the header only.
    let mut rooted_at_slash = false;

    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let path: Option<&str> = match shape {
            Shape::Find => {
                if listing_line_is_path(trimmed) {
                    rooted_at_slash |= trimmed.starts_with('/');
                    written.push(trimmed);
                    Some(normalise_path(trimmed))
                } else {
                    // `find: '…': Permission denied` and other non-path
                    // lines — keep them instead of silently dropping.
                    unmatched.push(line.to_string());
                    None
                }
            }
            Shape::Tree => {
                // Tree lines have a leading prefix of `│ `, `├── `, `└── `.
                // The file/dir name comes after the last `── `.
                if let Some((_, name)) = trimmed.rsplit_once("── ") {
                    // Ignore directories (commonly end with `/` in tree -F mode,
                    // or no extension at all). To be safe, count everything as
                    // a file when it has an extension; otherwise skip.
                    let n = name.trim_end_matches('/');
                    if !n.is_empty() && n.contains('.') {
                        Some(n)
                    } else {
                        None
                    }
                } else {
                    // No tree prefix at all → not tree output (a mixed doc);
                    // structure-only lines (bare dir names, `N directories`)
                    // are tree noise. Keep the foreign lines.
                    if !trimmed.contains("── ")
                        && !trimmed.starts_with('│')
                        && !trimmed.ends_with(" directories")
                        && !trimmed.ends_with(" files")
                        && trimmed.split_whitespace().count() > 1
                    {
                        unmatched.push(line.to_string());
                    }
                    None
                }
            }
            Shape::LsLaR => {
                // ls -laR lines: `-rw-r--r-- 1 user user 123 Jan 15 10:00 name`.
                // Parse by splitting whitespace; basename is the LAST field.
                // Skip directory-header lines (`./foo:`) and `total N` lines.
                if trimmed.starts_with('-')
                    && (trimmed.starts_with("-rw") || trimmed.starts_with("-r-"))
                {
                    trimmed.split_whitespace().last()
                } else {
                    let is_ls_noise = trimmed.ends_with(':')
                        || trimmed.starts_with("total ")
                        || trimmed.starts_with('d')
                        || trimmed.starts_with('l')
                        || trimmed.is_empty();
                    if !is_ls_noise {
                        unmatched.push(line.to_string());
                    }
                    None
                }
            }
        };

        let Some(path) = path else {
            continue;
        };
        if path.is_empty() {
            continue;
        }

        paths.push(path);
    }

    // The shared root, and why grouping without it says nothing.
    //
    // `split_top_and_ext` keys on the FIRST segment. For `find .` that is
    // real (`crates`, `docs`, `tests`). For `find /abs/path` — the form this
    // repo's own instructions ask for — it is the first segment of an
    // absolute path, so every entry lands in one group called `home` and the
    // breakdown reports "1 top-level directory" for a listing spanning 88 of
    // them. Measured on a real 400-path listing: 88 directories, one group.
    //
    // UNCAPPED, and that was measured rather than assumed. Capping at
    // `segments - 1` of the shortest entry looks right — a set of files does
    // not share a directory through its filenames — but `find /one/deep`
    // lists its own root as the first line, and that line then drags the
    // whole grouping one level shallower: `src` and `docs` collapse back
    // into a single group `deep`. The cases the cap was written for cost
    // nothing without it: a path consumed entirely leaves no segment below
    // the root, which `split_top_and_ext_below` already renders `(root)` —
    // exactly what the capped form produced for them.
    // **A list that fits ships, grouped — not as a census.**
    // `grep -rIli statistiques ~/projects | head -50` (sub-agent transcript,
    // 2026-09-29): fifty paths, 6 KB, rendered "50 file(s) across 2
    // directories — no individual path is listed"; the next call was
    // `expand-ref`, two times out of two. The paths WERE the answer. The
    // pipe sends the output down the auto route, which only knew the
    // census, whereas the same command without `| head` goes through the
    // grouped rendering of `git_listing` (common prefix said once, each
    // directory once, nothing lost). That rendering first, as long as it
    // fits under the shipping threshold; the census beyond.
    // A `find: '…': Permission denied` among the paths used to send the
    // whole listing to the census: the lines that are not paths follow the
    // listing, verbatim, and the pair is held to the same threshold.
    if matches!(shape, Shape::Find) {
        if let Some(mut listed) = listed_whole(&written) {
            if !unmatched.is_empty() {
                listed.push_str("\n\n");
                listed.push_str(&unmatched.join("\n"));
            }
            if tk_base::delivery::fits(&listed) {
                return listed;
            }
        }
    }

    let skip = common_path_segments(&paths);
    let shared_root = if skip == 0 {
        String::new()
    } else {
        paths[0].split('/').take(skip).collect::<Vec<_>>().join("/")
    };

    for path in &paths {
        total_files += 1;
        let (top, ext) = split_top_and_ext_below(path, skip);
        *by_dir.entry(top.clone()).or_insert(0) += 1;
        if let Some(e) = ext {
            *by_ext.entry(e.clone()).or_insert(0) += 1;
            *by_dir_ext.entry(top).or_default().entry(e).or_insert(0) += 1;
        }
    }

    // ---- Render ----
    let mut out = String::with_capacity(2048);
    let shape_label = match shape {
        Shape::Find => "find",
        Shape::Tree => "tree",
        Shape::LsLaR => "ls -laR",
    };
    let n_dirs = by_dir.len();

    if total_files == 0 {
        // Nothing parsed as a file entry — `out` is still EMPTY here, and
        // returning it replaced the ENTIRE input with an empty string (a
        // tree of bare directories, or a doc the loose detector grabbed).
        // No compaction is possible: hand back the original.
        return text.to_string();
    }

    // Top summary — restored after the L122 banner-strip pass.
    let _ = writeln!(
        out,
        "{shape_label}, {total_files} file(s) across {n_dirs} director{} \
         — a rollup: no individual path is listed below. Narrow the command \
         (a subdirectory, or a `-name` or glob filter) to see them.",
        if n_dirs == 1 { "y" } else { "ies" }
    );
    // On its OWN line, never inside the sentence. The redaction pass runs
    // after this function and scrubs a path under `$HOME` to nothing, which
    // mid-sentence reads as `400 file(s) under  across 3 directories`.
    if !shared_root.is_empty() {
        let _ = writeln!(
            out,
            "shared root: {}{shared_root}/",
            if rooted_at_slash { "/" } else { "" }
        );
    }

    // Per top-level directory breakdown, sorted by file count desc.
    let mut dirs_sorted: Vec<_> = by_dir.iter().collect();
    dirs_sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

    out.push_str("\nbreakdown by directory:\n");
    for (dir, count) in dirs_sorted.iter().take(TOP_DIRS_CAP) {
        // Per-dir extension top-5.
        let ext_summary = match by_dir_ext.get(*dir) {
            Some(exts) => {
                let mut v: Vec<_> = exts.iter().collect();
                if v.is_empty() {
                    String::new()
                } else {
                    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                    let top5: Vec<_> = v.iter().take(5).map(|(e, c)| format!("{e}: {c}")).collect();
                    format!("  ({})", top5.join(", "))
                }
            }
            None => String::new(),
        };
        let _ = writeln!(out, "  {:<32} {} file(s){}", dir, count, ext_summary);
    }
    if dirs_sorted.len() > TOP_DIRS_CAP {
        let _ = writeln!(
            out,
            "  … {} more directories elided",
            dirs_sorted.len() - TOP_DIRS_CAP
        );
    }

    // Global extension breakdown.
    let mut ext_sorted: Vec<_> = by_ext.iter().collect();
    ext_sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

    let _ = write!(out, "\nby extension ({} distinct):\n", ext_sorted.len());
    for (ext, count) in ext_sorted.iter().take(EXTENSIONS_CAP) {
        let pct = (**count as f64 * 100.0) / (total_files as f64);
        let _ = writeln!(out, "  {ext:<10} {count:>6}  ({pct:>5.1}%)");
    }
    if ext_sorted.len() > EXTENSIONS_CAP {
        let _ = writeln!(
            out,
            "  … {} more extension(s) elided",
            ext_sorted.len() - EXTENSIONS_CAP
        );
    }

    if !unmatched.is_empty() {
        out.push('\n');
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
    }
    out.trim_end().to_string()
}

/// Every path, the common prefix said once and each directory once — when
/// that rendering fits under the shipping threshold.
pub(crate) fn listed_whole(paths: &[&str]) -> Option<String> {
    let mut out = String::new();
    let strip = match common_prefix_header(paths) {
        Some((header, strip)) => {
            out.push_str(&header);
            out.push('\n');
            strip
        }
        None => 0,
    };
    push_grouped(&mut out, paths, strip, &|l| Some(l));
    let out = out.trim_end().to_string();
    tk_base::delivery::fits(&out).then_some(out)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// Group `paths` by the SHALLOWEST directory depth that actually tells them
/// apart, and render `dir/ ×N` pairs.
///
/// The census used to take `path.split('/').next()` — the top-level segment,
/// always. MEASURED 2026-08-04: `find crates -name "*.rs"` summarised 1 092
/// paths as `crates/ ×1092`, and giving `find` two roots under `crates/` still
/// produced one bucket. Every path shares the search root by construction, so
/// the summary restated the total and named the directory the caller had just
/// typed. Same defect the `grep` banner had that morning: a summary that repeats
/// its input.
///
/// Depth grows only until the paths separate, so a listing whose top level
/// already discriminates keeps exactly the census it printed before. Capped at
/// four segments (deeper reads as a file list, not a summary) and at eight
/// buckets, with the remainder counted rather than dropped.
pub fn path_census(paths: &[&str]) -> String {
    use std::collections::HashMap;
    const MAX_BUCKETS: usize = 8;
    let bucket_at = |depth: usize| -> HashMap<String, usize> {
        let mut m: HashMap<String, usize> = HashMap::new();
        for p in paths {
            let segs: Vec<&str> = p.split('/').collect();
            // A path shorter than `depth` is its own bucket: it cannot be
            // grouped any deeper than it goes.
            let take = depth.min(segs.len().saturating_sub(1).max(1));
            *m.entry(segs[..take].join("/")).or_insert(0) += 1;
        }
        m
    };
    // Bucket at the first level BELOW the root every path shares.
    //
    // This used to start at depth 1 and deepen while the level failed to
    // separate, stopping at the first one that did not. That is right for a
    // listing which branches early and wrong for one that does not branch for
    // a while: `find /home/u/proj -name '*.rs'` has four unproductive levels
    // (``, `home`, `u`, `proj`) before anything differs, so the loop quit
    // after one and the census read `/ ×2800` — the total the reader already
    // had. That is the exact failure the relative-root test below was written
    // for in 2026-08-04, never extended to the absolute root. Measured on ten
    // real trees: ten said `/ ×N`.
    //
    // The loop is GONE rather than kept as defence, and that was measured
    // too. `common_path_segments` returns the LONGEST shared prefix, so two
    // paths differ at this level by construction and there is nothing left to
    // deepen towards; the one case that still yields a single bucket — every
    // path identical — is one no depth can separate. Disabling the loop left
    // the census byte-identical on thirteen real listings.
    let skip = crate::filelist::common_path_segments(paths);
    let counts = bucket_at(skip + 1);
    let mut list: Vec<(String, usize)> = counts.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let shown: Vec<String> = list
        .iter()
        .take(MAX_BUCKETS)
        .map(|(d, n)| format!("{d}/ \u{d7}{n}"))
        .collect();
    let mut s = shown.join(", ");
    if list.len() > MAX_BUCKETS {
        let rest: usize = list[MAX_BUCKETS..].iter().map(|(_, n)| n).sum();
        s.push_str(&format!(
            ", +{} other dir(s) \u{d7}{rest}",
            list.len() - MAX_BUCKETS
        ));
    }
    s
}

#[cfg(test)]
include!("filelist_tests.rs");
