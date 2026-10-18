//! Archive-listing compaction: `tar -tvf`, `unzip -l`, `jar tf`, `7z l`.
//!
//! Agents routinely receive listings of multi-thousand-entry tarballs (CI
//! release tarballs, container layer dumps, JAR/WAR contents). The signal
//! they care about is the **paths**; the per-line repeated chrome
//! (permissions, owner/group, size, date, time) is pure noise.
//!
//! [`compact`] auto-detects the format and emits a clean path-only listing,
//! prefixed by a one-line header summarising what was dropped:
//!
//! ```text
//! [tokenade:archive] tar-listing — 4823 entries, columns dropped: perm/owner/size/date
//!   src/
//!   src/main.rs
//!   src/lib.rs
//!   ...
//! ```
//!
//! Long listings beyond [`MAX_ENTRIES`] get folded into per-top-level-dir
//! counts (`src/  (123 files)`) so a 100k-entry archive still fits in a few
//! hundred tokens.

use std::fmt::Write;

/// Beyond this many entries we group by top-level directory.
pub const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `tar -tvf` style: `-rw-r--r-- user/group 1234 2024-01-01 12:34 path`.
    TarVerbose,
    /// `unzip -l` style: header row + `length date time name`.
    UnzipList,
    /// `7z l` style: similar tabular with explicit headers.
    SevenZipList,
    /// Plain paths (already compact — pass-through).
    Plain,
    /// Not detected as any of the above.
    Unknown,
}

pub fn detect_format(text: &str) -> Format {
    let lines: Vec<&str> = text.lines().take(10).collect();
    if lines.is_empty() {
        return Format::Unknown;
    }
    // `unzip -l` has a fixed header line.
    if lines.iter().any(|l| {
        let t = l.trim();
        t.starts_with("Length") && t.contains("Date") && t.contains("Time") && t.contains("Name")
    }) {
        return Format::UnzipList;
    }
    // 7-zip has a `Date Time Attr Size Compressed Name` header.
    if lines.iter().any(|l| {
        l.contains("Date") && l.contains("Time") && l.contains("Attr") && l.contains("Name")
    }) {
        return Format::SevenZipList;
    }
    // `tar -tvf` has lines like `drwxr-xr-x user/group 0 2024-01-01 path/`.
    // `ls -l`/`ls -la` output shares this exact shape — but tar NEVER prints
    // a `total N` header nor `.`/`..` entries, and a real tar listing is
    // homogeneous (every line is an entry). Field incident 2026-07: an
    // `ls -la` section inside a multi-section `tokenade exec` output was
    // classified TarVerbose and the lossy compactor dropped every other
    // section. Require the entry lines to dominate AND no ls signature.
    let nonempty: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let tarish = nonempty.iter().filter(|l| is_tar_v_line(l)).count();
    let ls_signature = nonempty.iter().any(|l| {
        let t = l.trim();
        let total_hdr = t
            .strip_prefix("total ")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
        // `.`/`..` name = the LAST whitespace token (ls has 8 columns before
        // the name, tar 5 — a column-skipping extractor misses ls's name).
        let dot_entry =
            is_tar_v_line(l) && matches!(l.split_whitespace().last(), Some(".") | Some(".."));
        // `ls -l` piped through `grep -v '^total'` keeps neither signature
        // above, and was folded to bare names under a header announcing the
        // sizes dropped — on output whose sizes and modes were what the
        // listing was run for. Its second column is a link count, never 0 for
        // an existing file; GNU tar writes `owner/group` there, bsdtar `0`.
        total_hdr || dot_entry || is_ls_long_line(l)
    });
    if tarish >= 2 && !ls_signature && tarish * 10 >= nonempty.len() * 7 {
        return Format::TarVerbose;
    }
    // Plain path listing — at least 2 lines that look like a relative path
    // (no leading spaces, contain `/`, no obvious column structure). We
    // intentionally require the `/` (rather than `.`) to avoid false-positive
    // on prose containing a sentence period.
    //
    // Strict reject of `=`, `{`, `}`, `"` and HTTP/RFC tokens stops the
    // detector from grabbing access logs (which contain `/path` inside
    // the request-line quote) and Prometheus metrics (which use
    // `{label="val"}` selectors). Both have their own dedicated
    // compactors that beat the path-listing one.
    // L78 — early-reject: presence of a command-chain section
    // header anywhere in the first 5 lines means this isn't a path
    // listing, it's a multi-section Bash dump. The chain compactor
    // owns it.
    for l in lines.iter().take(5) {
        let t = l.trim();
        if (t.starts_with("===") && t.ends_with("===") && t.len() > 6)
            || (t.starts_with("---") && t.ends_with("---") && t.len() > 6)
            || (t.starts_with("###") && t.len() > 4)
        {
            return Format::Unknown;
        }
    }
    let pathish = lines
        .iter()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty()
                // A COMMENT is not a path, and its own marker is what made it
                // look like one: `//` contains a slash. Measured 2026-08-26 on
                // `basemind/crates/stack-graphs/src/graph.rs` — its first ten
                // lines are a licence header and a `//!` module doc, seven of
                // the nine non-empty ones counted as paths, and the file was
                // delivered as `[tokenade:archive] plain-listing — 1412
                // entries` with its doc comments dropped. 2 287 source files
                // in this corpus were claimed the same way.
                //
                // Narrow ON PURPOSE to the slash-comment markers, because the
                // slash IS the mechanism: `//` and `/*` satisfy the `contains('/')`
                // test with their own marker and nothing else. A first pass also
                // rejected `--` and `*`; neither is justified by the evidence and
                // `--` broke a real case — pip's progress bar is
                // `---------- 123.4/123.4 kB 14.2 MB/s eta 0:00:00`, which opens
                // on four dashes and is not a comment. `##` was already rejected
                // below, for this same reason.
                && !t.starts_with("//")
                && !t.starts_with("/*")
                && !t.contains("  ")
                // The line must BE a path, not merely CONTAIN one. Every
                // repair of this predicate so far bolted on another substring
                // exception — `//` comments (2 287 source files claimed), `##`,
                // ` HTTP/` — because `contains('/')` asks the wrong question.
                // A listing prints the path LAST: `find`, `ls -R`, `rsync`,
                // `du -h` (`4.0K\t./path`). Prose that mentions a path does
                // not: `processing /some/path/1/module-1.ts with 7 symbols and
                // 1 warnings` ends on `warnings`. Measured 2026-08-28: 4 000
                // such lines were delivered as `plain-listing — 4001 entries`
                // grouped into `processing / (4000 entries)`, where the
                // numeric-shape compactor renders the whole set as one line
                // carrying the pattern AND its statistics.
                && t.split_whitespace().next_back().is_some_and(|w| w.contains('/'))
                && t.chars().all(|c| c.is_ascii_graphic() || c == ' ')
                && !t.contains('=')
                && !t.contains('"')
                && !t.contains('{')
                && !t.contains('}')
                && !t.contains(" HTTP/")
                && !t.contains("] \"")
                && !t.starts_with("##[")
                && !t.starts_with("##")
        })
        .count();
    let non_empty = lines.iter().filter(|l| !l.trim().is_empty()).count();
    // L78 — tighter thresholds: require ≥5 path-shaped lines (was 2)
    // AND ≥80 % of non-empty lines must match (was 50 %). Stops the
    // detector from claiming the output of a multi-command Bash
    // (which happens to mention a couple of paths in echoes) as an
    // archive listing.
    if pathish >= 5 && pathish * 5 >= non_empty * 4 && entries_group_by_directory(text) {
        return Format::Plain;
    }
    Format::Unknown
}

/// Does this set of path-shaped lines actually GROUP? A listing shares
/// directories — that is what makes it a listing, and it is what the
/// `plain-listing` renderer promises when it says "N entries grouped by
/// directory".
///
/// Every repair of the shape predicate above has been another substring
/// exception bolted onto `contains('/')`, because `/` is a character other
/// things use. Base64 uses it as one of its 64 symbols: measured on 200 KB of
/// random bytes, `base64` prints 3 509 lines whose last token contains a
/// slash, and every one of the exclusions above lets them through. They were
/// delivered as **`[tokenade:archive] plain-listing — 3509 entries`** followed
/// by "(3509 entries grouped by directory — pass a path or glob to the listing
/// to see individual entries)": entries that are not entries, directories that
/// do not exist, and advice that means nothing. Not a worse fold — false
/// facts, the version-folding defect one module over.
///
/// Asked of the WHOLE text, deliberately. `detect_format` decides on
/// `text.lines().take(10)` — a ten-line sample — and on ten random base64
/// lines the answer is a coin toss: eight of the first ten carried a slash,
/// which cleared the 80 % shape bar, and ten entries are below [`MAX_ENTRIES`]
/// so grouping cannot be judged there at all. A guard next to a sampling
/// detector sees the sample, not the data.
///
/// The separation is not close. Distinct first segments over path-shaped
/// lines, measured the same day: `find crates -type f` 0.001, `du -a` 0.000,
/// `git ls-files` 0.005, `ls -lR` 0.011, 500 absolute paths under different
/// trees 0.002 (they share the empty segment before the leading `/`), a mixed
/// rsync listing 0.005 — against **0.969** for base64. The bar is a half, the
/// same shape of bar already put on versions, and no real listing measured comes
/// within fifty times of it.
fn entries_group_by_directory(text: &str) -> bool {
    let mut roots: std::collections::HashSet<&str> = Default::default();
    let mut n = 0usize;
    for l in text.lines() {
        let t = l.trim();
        let Some(last) = t.split_whitespace().next_back() else {
            continue;
        };
        let Some((root, _)) = last.split_once('/') else {
            continue;
        };
        n += 1;
        roots.insert(root);
    }
    if n == 0 {
        return false;
    }
    // A handful of entries genuinely can be almost one root apiece:
    // `src/main.rs`, `tests/e2e/mod.rs`, `docs/guide/intro.md` is three roots
    // for five entries and is a listing. Below `MIN_TO_JUDGE` the ratio says
    // nothing and the renderer prints every entry verbatim anyway, claiming
    // nothing about directories.
    //
    // An ABSOLUTE floor, not a fraction of the input, because the two callers
    // hand this different amounts: `detect_format` is also called on a
    // truncated head by `auto::detectors`. A floor at [`MAX_ENTRIES`] made the
    // guard vacuous there — the head holds fewer lines than that — and the
    // same base64 blob came back `[disclose:summary kind=ls.txt]` through the
    // content route while the named route had been fixed. A guard whose bar
    // scales with the caller's window is a guard the caller can switch off by
    // passing less.
    const MIN_TO_JUDGE: usize = 20;
    if n < MIN_TO_JUDGE {
        return true;
    }
    roots.len() * 2 <= n
}

/// An `ls -l` line: a mode block, then a LINK COUNT — never 0 for an
/// existing file, where GNU tar writes `owner/group` and bsdtar `0`. One
/// declarer for the two readers that must agree on it: tar refuses these
/// lines, and the file-list detector takes them when `ls -l` was given files
/// by name and printed no `total` line (see `filelist::detect`).
pub(crate) fn is_ls_long_line(line: &str) -> bool {
    is_tar_v_line(line)
        && line
            .split_whitespace()
            .nth(1)
            .is_some_and(|t| t.bytes().all(|b| b.is_ascii_digit()) && t != "0")
}

fn is_tar_v_line(line: &str) -> bool {
    // Common shape: `[bcdlsp-]rwxrwxrwx ... [path]`. We check the permissions
    // prefix AND that the entry carries the trailing owner/size/date/path.
    let bytes = line.as_bytes();
    let perm_ok = bytes.len() >= 10
        && matches!(bytes[0], b'-' | b'd' | b'l' | b'c' | b'b' | b's' | b'p')
        && bytes[1..10].iter().all(|b| {
            matches!(
                b,
                b'r' | b'w' | b'x' | b'-' | b's' | b't' | b'S' | b'T' | b'l'
            )
        });
    if !perm_ok {
        return false;
    }
    // The permission block is separated from the owner/group column by
    // whitespace, and a real entry always carries an alphanumeric field
    // (owner, size, or path). A table-separator line — e.g. sqlite `-column`
    // mode or a markdown rule like `----------  ------  -----` — has byte 0 ==
    // `-` and bytes 1..10 all `-` (so `perm_ok` passes) but is ALL dashes and
    // spaces with no alphanumerics. Requiring one rejects those separators,
    // which previously let two stacked tables read as a 2-entry tar listing.
    bytes.len() > 10
        && matches!(bytes[10], b' ' | b'\t')
        && line.bytes().any(|b| b.is_ascii_alphanumeric())
}

pub fn compact(text: &str) -> String {
    let fmt = detect_format(text);
    match fmt {
        Format::TarVerbose => compact_tar_verbose(text),
        Format::UnzipList => compact_unzip(text),
        Format::SevenZipList => compact_7z(text),
        Format::Plain => compact_paths(text, "plain-listing"),
        Format::Unknown => text.to_string(),
    }
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

fn compact_tar_verbose(text: &str) -> String {
    // Fold RUNS of entry lines; every other line passes through VERBATIM.
    // A compactor must never destroy content it did not recognise — the
    // pre-fix version dropped non-entry lines, which erased the other
    // sections of a mixed script output (field incident 2026-07).
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    for line in text.lines() {
        if is_tar_v_line(line) {
            // Path is everything after the (date time) pair — 6th field on.
            if let Some(p) = path_from_tar_line(line) {
                run.push(p);
                continue;
            }
        }
        flush_tar_run(&mut run, &mut out);
        out.push(line.to_string());
    }
    flush_tar_run(&mut run, &mut out);
    out.join("\n")
}

fn flush_tar_run(run: &mut Vec<String>, out: &mut Vec<String>) {
    if run.is_empty() {
        return;
    }
    out.push(render_paths(
        "tar-listing — perm/owner/size/date dropped",
        run,
    ));
    run.clear();
}

/// The name in an entry line is whatever follows its time (`12:34`) or year
/// (`2023`) column.
///
/// It used to skip a fixed count of token STARTS and stop one byte into the
/// fifth, so GNU `tar -tvf` came back as `2:34 src/a.rs` — and an `ls -l`
/// (or bsdtar) line, three columns wider, as its size minus the first digit
/// followed by the date: `ls -la ~/.ssh` delivered `05  2 oct. 15:30` for a
/// 105-byte file under a header saying the size was dropped. Both shapes are
/// read here: GNU tar's `owner/group` second column starts the search at the
/// date, the link count of `ls`/bsdtar starts it past `owner group size`, so a
/// four-digit size is never taken for a year. No such column ⇒ `None`, and
/// the line passes through verbatim.
fn path_from_tar_line(line: &str) -> Option<String> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, c) in line
        .char_indices()
        .chain(std::iter::once((line.len(), ' ')))
    {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                spans.push((s, i));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    let token = |k: usize| &line[spans[k].0..spans[k].1];
    if spans.len() < 2 {
        return None;
    }
    let first_date_col = if token(1).contains('/') { 3 } else { 5 };
    let is_time_or_year = |t: &str| {
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let mut parts = t.split(':');
        let head = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            t.len() == 4 && digits(t)
        } else {
            head.len() <= 2
                && digits(head)
                && rest.len() <= 2
                && rest.iter().all(|r| r.len() == 2 && digits(r))
        }
    };
    let at =
        (first_date_col..spans.len().saturating_sub(1)).find(|&k| is_time_or_year(token(k)))?;
    let name = line[spans[at + 1].0..].trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn compact_unzip(text: &str) -> String {
    // Fold the `---`-delimited entry body; every line OUTSIDE it passes
    // through verbatim (same principle as compact_tar_verbose — a section
    // following the listing must survive).
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let mut in_body = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("---") {
            if in_body {
                flush_run(
                    &mut run,
                    &mut out,
                    "unzip-listing — length/date/time dropped",
                );
            }
            in_body = !in_body;
            continue;
        }
        if in_body {
            // Layout: `   12345  2024-01-01 12:34   path`
            if let Some(name) = t.splitn(4, char::is_whitespace).nth(3) {
                run.push(name.trim().to_string());
                continue;
            }
        }
        out.push(line.to_string());
    }
    flush_run(
        &mut run,
        &mut out,
        "unzip-listing — length/date/time dropped",
    );
    out.join("\n")
}

fn flush_run(run: &mut Vec<String>, out: &mut Vec<String>, label: &str) {
    if run.is_empty() {
        return;
    }
    out.push(render_paths(label, run));
    run.clear();
}

fn compact_7z(text: &str) -> String {
    // Same passthrough discipline as compact_unzip.
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let mut in_body = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("------") {
            if in_body {
                flush_run(&mut run, &mut out, "7z-listing — date/attr/size dropped");
            }
            in_body = !in_body;
            continue;
        }
        if in_body {
            // `2024-01-01 12:34:56 ....A 12345 6789 path`
            if let Some(name) = t.split_whitespace().nth(5) {
                let idx = t.find(name).unwrap_or(0);
                run.push(t[idx..].trim().to_string());
                continue;
            }
        }
        out.push(line.to_string());
    }
    flush_run(&mut run, &mut out, "7z-listing — date/attr/size dropped");
    out.join("\n")
}

fn compact_paths(text: &str, label: &str) -> String {
    let paths: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    if paths.len() > MAX_ENTRIES && paths.iter().any(|p| is_section_header(p)) {
        return render_sections(label, &paths);
    }
    render_paths(label, &paths)
}

/// `for d in a b c; do echo "=== $d"; find $d -type f; done` — a line an
/// `echo` wrote to say where the next paths come from. Not a path: it starts
/// with a run of `=`, `#`, `-`, `*` or `>` followed by a space.
fn is_section_header(line: &str) -> bool {
    let t = line.trim_start();
    let run = t
        .bytes()
        .take_while(|b| matches!(b, b'=' | b'#' | b'-' | b'*' | b'>'))
        .count();
    run >= 2 && t.as_bytes().get(run) == Some(&b' ')
}

/// A listing cut into sections by `echo` headers, each grouped on its own.
///
/// Grouped as one listing, the headers were paths like any other: three
/// `=== crates/…` lines became a directory `=== crates/  (3 entries)` and the
/// three sections' paths were pooled, so which entries the loop found under
/// which argument — the thing the loop was written to show — was gone.
fn render_sections(label: &str, lines: &[String]) -> String {
    let n = lines.iter().filter(|l| !is_section_header(l)).count();
    let mut out = format!(
        "[tokenade:archive] {label} — {n} entries\n  ({n} entries grouped by directory, \
         under each section header — pass a path or glob to the listing to see individual entries)\n"
    );
    let mut section: Vec<&str> = Vec::new();
    let flush = |section: &mut Vec<&str>, out: &mut String| {
        if section.is_empty() {
            return;
        }
        let trimmed: Vec<&str> = section.iter().map(|p| p.trim_start_matches('/')).collect();
        let skip = crate::filelist::common_path_segments(&trimmed);
        let mut prefix: String = trimmed[0]
            .split('/')
            .take(skip)
            .map(|s| format!("{s}/"))
            .collect();
        if skip > 0 && section[0].starts_with('/') {
            prefix.insert(0, '/');
        }
        let entries: Vec<(Vec<&str>, &str)> = section
            .iter()
            .zip(&trimmed)
            .map(|(p, t)| (t.split('/').skip(skip).collect(), *p))
            .collect();
        group_lines(&entries, &prefix, 0, out);
        section.clear();
    };
    for line in lines {
        if is_section_header(line) {
            flush(&mut section, &mut out);
            let _ = writeln!(out, "{}", line.trim_end());
        } else {
            section.push(line);
        }
    }
    flush(&mut section, &mut out);
    out.trim_end().to_string()
}

fn render_paths(label: &str, paths: &[String]) -> String {
    let n = paths.len();
    let mut out = format!("[tokenade:archive] {label} — {n} entries\n");
    if n <= MAX_ENTRIES {
        for p in paths {
            out.push_str("  ");
            out.push_str(p);
            out.push('\n');
        }
        return out.trim_end().to_string();
    }
    // Group by top-level directory.
    //
    // Both maps are filled in ONE pass, and the pass is linear on purpose.
    // This grouping only runs past MAX_ENTRIES, i.e. exactly when the listing
    // is large, and it used to do two O(n) scans per entry — `roots.contains`
    // and, below, a `paths.iter().find` per single-entry root. On a listing
    // whose entries are all distinct (every line its own root, which is what
    // a misdetected source file looks like) that is n² twice over: MEASURED
    // 2 507 ms for 1,28 Mo, ~26x per 4x of input.
    let trimmed: Vec<&str> = paths.iter().map(|p| p.trim_start_matches('/')).collect();
    let skip = crate::filelist::common_path_segments(&trimmed);
    let shared_root = if skip == 0 {
        String::new()
    } else {
        trimmed[0]
            .split('/')
            .take(skip)
            .collect::<Vec<_>>()
            .join("/")
    };
    let entries: Vec<(Vec<&str>, &str)> = paths
        .iter()
        .zip(&trimmed)
        .map(|(p, t)| (t.split('/').skip(skip).collect(), p.as_str()))
        .collect();

    let _ = writeln!(
        out,
        "  ({n} entries grouped by directory — pass a path or glob to the \
         listing to see individual entries)"
    );

    if !shared_root.is_empty() {
        let _ = writeln!(out, "  shared root: {shared_root}/");
    }
    group_lines(&entries, "", 0, &mut out);
    out.trim_end().to_string()
}

/// How deep [`group_lines`] may open a dominant directory, and how many
/// groups an opened directory may show before it stays one line.
const OPEN_DEPTH: usize = 4;
const OPEN_MAX_GROUPS: usize = 40;

/// One line per directory under `prefix`, in first-seen order — except the
/// directory that holds MOST of the entries, which is opened instead.
///
/// `find src -type f` on a 749-file project came back as `src/  (698
/// entries)` beside `scripts/  (50 entries)`: a line that
/// restates the total and says nothing of what the listing was run to show.
/// Across delivered results the rollup was followed by `expand-ref` five
/// times in thirteen. A directory holding more than half of what is listed
/// is the one the reader needs the inside of, so it is broken down a level
/// (up to [`OPEN_DEPTH`]), as long as that adds at most [`OPEN_MAX_GROUPS`]
/// lines.
fn group_lines(entries: &[(Vec<&str>, &str)], prefix: &str, depth: usize, out: &mut String) {
    let total = entries.len();
    let mut by_dir: std::collections::HashMap<&str, Vec<usize>> = Default::default();
    let mut roots: Vec<&str> = Vec::new();
    let mut loose: Vec<&str> = Vec::new();
    for (i, (rest, path)) in entries.iter().enumerate() {
        if rest.len() <= 1 {
            loose.push(path);
            continue;
        }
        let members = by_dir.entry(rest[0]).or_default();
        if members.is_empty() {
            roots.push(rest[0]);
        }
        members.push(i);
    }
    for root in roots {
        let members = &by_dir[root];
        if members.len() == 1 {
            let _ = writeln!(out, "  {}", entries[members[0]].1);
            continue;
        }
        if depth < OPEN_DEPTH && members.len() * 2 > total {
            let inner: Vec<(Vec<&str>, &str)> = members
                .iter()
                .map(|&i| (entries[i].0[1..].to_vec(), entries[i].1))
                .collect();
            let groups: std::collections::HashSet<&str> = inner
                .iter()
                .filter(|(r, _)| r.len() > 1)
                .map(|(r, _)| r[0])
                .collect();
            if !groups.is_empty() && groups.len() <= OPEN_MAX_GROUPS {
                group_lines(&inner, &format!("{prefix}{root}/"), depth + 1, out);
                continue;
            }
        }
        let _ = writeln!(out, "  {prefix}{root}/  ({} entries)", members.len());
    }

    const LOOSE_VERBATIM: usize = 3;
    if loose.len() <= LOOSE_VERBATIM {
        for e in &loose {
            let _ = writeln!(out, "  {e}");
        }
    } else if prefix.is_empty() {
        let _ = writeln!(out, "  (root)/  ({} entries)", loose.len());
    } else {
        let _ = writeln!(out, "  {prefix}  ({} files directly in it)", loose.len());
    }
}
#[cfg(test)]
include!("archive_tests.rs");
