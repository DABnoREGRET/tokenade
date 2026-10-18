//! `du` + `df` disk-usage output compaction.
//!
//! `du -h` / `du -sh *` / `du -h --max-depth=N` emit one line per directory
//! entry:
//!
//! ```text
//! 4.0K    ./Cargo.toml
//! 8.0K    ./README.md
//! 24K     ./src
//! 156M    ./vendor
//! 1.2G    ./target/release
//! 4.5G    ./target
//! 5.6G    .
//! ```
//!
//! On a real repo a full recursive `du -h` is 10 000 – 100 000 lines. The
//! agent reading it actually wants:
//!
//!   - **largest entries** sorted by size (top-N)
//!   - **total** size summary
//!   - **count** of entries
//!
//! Drops: the long tail of small directories, the verbose path repetition
//! when most of the tree is sub-KB junk.

use std::fmt::Write;

pub const TOP_ENTRIES_CAP: usize = 60;

pub fn detect(text: &str) -> bool {
    detect_du(text) || detect_df(text)
}

fn detect_du(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);
    let mut du_lines = 0usize;
    let mut total_lines = 0usize;
    for line in head.lines().take(80) {
        if line.trim().is_empty() {
            continue;
        }
        total_lines += 1;
        if parse_du_line(line).is_some() {
            du_lines += 1;
        }
    }
    du_lines >= 10 && total_lines > 0 && (du_lines * 100) / total_lines >= 80
}

/// L81 — `df` / `df -h` / `df -hT` detector.
///
/// Recognised by the column header line that starts with `Filesystem`
/// and contains both `Use%` and `Mounted` somewhere in the same row.
/// Tolerates locale variants (`Sys. de fichiers` etc.) by also
/// accepting any header line where the trailing tokens match
/// `Use% Mounted on` / `Capacity Mounted`.
pub fn detect_df(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(2_048)).unwrap_or(text);
    let mut data_lines = 0usize;
    let mut saw_header = false;
    for line in head.lines().take(40) {
        if line.trim().is_empty() {
            continue;
        }
        if !saw_header && is_df_header(line) {
            saw_header = true;
            continue;
        }
        if saw_header && parse_df_line(line).is_some() {
            data_lines += 1;
        }
    }
    // Require the header AND at least 3 data lines that parse — guards
    // against a single-line `df` (`df /` with no trailing rows) being
    // misclassified.
    saw_header && data_lines >= 3
}

fn is_df_header(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    // The use-% column has many locale variants. We list the common
    // ones (en/fr/de/es/it/pt/ru) and key on the `%`-suffix shape,
    // which is locale-invariant.
    let has_use = lower.contains("use%")
        || lower.contains("uti%")          // French (Uti%)
        || lower.contains("verw%")         // German
        || lower.contains("uso%")          // Spanish / Portuguese
        || lower.contains("capacity")
        || lower.contains("util%"); // Italian / RU romanised
                                    // Mount-point column word.
    let has_mount = lower.contains("mounted")
        || lower.contains("monté")         // French (Monté sur)
        || lower.contains("eingehängt")    // German
        || lower.contains("montado")       // Spanish / Portuguese
        || lower.contains("montato"); // Italian
                                      // Filesystem column word.
    let has_fs_word = lower.starts_with("filesystem")
        || lower.contains("sys. de fichiers")  // French
        || lower.contains("dateisystem")       // German
        || lower.contains("sistema de archi"); // Spanish prefix
    (has_use && has_mount) && (has_fs_word || lower.contains("size") || lower.contains("taille"))
}

/// Parse one `df` data row into `(filesystem, size, used, avail,
/// use_pct, mount)`. Returns `None` for prose / column-misaligned
/// rows. We split on whitespace and take the last 5 tokens as the
/// size/used/avail/use%/mount block — that handles both numeric
/// (`134217728`) and human (`128M`) sizes AND `df -hT` (which adds
/// a Type column before Size).
pub struct DfRow<'a> {
    pub filesystem: &'a str,
    pub size: &'a str,
    pub used: &'a str,
    pub avail: &'a str,
    pub use_pct: &'a str,
    pub mount: &'a str,
}

fn parse_df_line(line: &str) -> Option<DfRow<'_>> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 6 {
        return None;
    }
    // Last token is mount path. Walk backward to confirm.
    let mount = *parts.last()?;
    if !mount.starts_with('/') && mount != "-" {
        // Some df rows have `none` or empty mount; reject as data
        // line.
        return None;
    }
    let use_pct = parts[parts.len() - 2];
    if !use_pct.ends_with('%') {
        return None;
    }
    let avail = parts[parts.len() - 3];
    let used = parts[parts.len() - 4];
    let size = parts[parts.len() - 5];
    // Filesystem name is everything before. With `df -hT` Type appears
    // between Filesystem and Size; we treat both columns as part of
    // "filesystem" — the agent doesn't typically need the type
    // separately.
    let fs_end = parts.len() - 5;
    if fs_end == 0 {
        return None;
    }
    let fs_str = &parts[..fs_end].join(" ")[..];
    // Leak into the original line span: easier path-wise to keep
    // string-borrow semantics by using the first token alone when
    // there was only one filesystem token, and joining otherwise.
    let filesystem = if fs_end == 1 {
        parts[0]
    } else {
        // SAFETY: we used the joined String above only for length
        // calc. Return the first token — adequate for the
        // compactor's needs (display the source disk name) and
        // keeps lifetimes simple.
        let _ = fs_str;
        parts[0]
    };
    Some(DfRow {
        filesystem,
        size,
        used,
        avail,
        use_pct,
        mount,
    })
}

/// True when this `df` row should be dropped from the compacted
/// output. Targets the noise that bloats every modern Linux desktop
/// `df -h`: 10–30 snap loopback mounts (one per installed snap), idle
/// tmpfs partitions, and `/dev/loop*` overlays. Real disks ALWAYS
/// survive.
fn is_df_noise(row: &DfRow<'_>) -> bool {
    let loop_fs = row.filesystem.starts_with("/dev/loop");
    let snap_mount = row.mount.starts_with("/snap/")
        || row.mount.contains("/snapd/snap/")
        || row.mount.starts_with("/var/lib/snapd/");
    let used_is_zero = row.used == "0" || row.used == "0B" || row.used == "0K" || row.used == "-";
    (loop_fs && snap_mount) || (row.filesystem == "tmpfs" && used_is_zero)
}

/// Parse a single `du` line into `(size_in_bytes, path)`. Lines that don't
/// match the `<size>{whitespace}<path>` pattern (e.g. `df -h` header rows,
/// stray prose, vmstat columns, psql ASCII tables) return `None`.
fn parse_du_line(line: &str) -> Option<(u64, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let (size_str, path_raw) = trimmed.split_once(char::is_whitespace)?;
    let path = path_raw.trim_start();
    if path.is_empty() {
        return None;
    }
    // Reject `df -h` filesystem rows where the first field is `/dev/...`.
    if size_str.starts_with('/') {
        return None;
    }
    // Real du paths are single tokens (possibly containing `/`) — they
    // do NOT contain internal multi-space padding (that's vmstat /
    // ps aux / psql columns) and never start with `|` (psql ASCII
    // table separators). We require the path either contains `/`
    // (real filesystem path) or is a single space-free identifier
    // (e.g. `du node_modules`).
    if path.starts_with('|') {
        return None;
    }
    if path.contains("  ") {
        return None;
    }
    // `du` separates the size from the path with ONE tab, so a du path never
    // contains a tab itself. Anything that does is a table with three or more
    // columns, and reading one as disk usage is not a near-miss: MEASURED on a
    // 200-row TSV of PR-shaped records, the output opened with `~214.9K total`
    // and `largest entries:` — two statements false of the input — re-sorted
    // every row DESCENDING BY ITS FIRST COLUMN, which destroys the order a
    // list exists to convey, and then elided the tail, where the one row
    // carrying unique content had been sorted to.
    //
    // The rule below already tried to say "a du path is one token", but it
    // waived the whitespace check for anything containing a `/` — and a table
    // column like `feat/br-2` contains one. The tab is the exact
    // discriminator: verified against real `du -a` output on a tree whose
    // directories carry SPACES (`dutest/mes documents/sous dossier`), where
    // not one path contains a tab.
    if path.contains('\t') {
        return None;
    }
    if !path.contains('/') && path.contains(char::is_whitespace) {
        return None;
    }
    parse_size(size_str).map(|s| (s, path))
}

/// Parse `du` size strings: `"123"` (bytes), `"4.0K"`, `"156M"`, `"1.2G"`,
/// `"2T"`. Case-insensitive on the suffix. Returns the size in bytes.
fn parse_size(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let last = *bytes.last()?;
    let (num_part, multiplier): (&str, u64) = match last {
        b'K' | b'k' => (&s[..s.len() - 1], 1024),
        b'M' | b'm' => (&s[..s.len() - 1], 1024 * 1024),
        b'G' | b'g' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        b'T' | b't' => (&s[..s.len() - 1], 1024_u64.pow(4)),
        b'P' | b'p' => (&s[..s.len() - 1], 1024_u64.pow(5)),
        b'B' | b'b' => (&s[..s.len() - 1], 1),
        d if d.is_ascii_digit() => (s, 1),
        _ => return None,
    };
    let num: f64 = num_part.parse().ok()?;
    if !num.is_finite() || num < 0.0 {
        return None;
    }
    Some((num * multiplier as f64) as u64)
}

/// Render a byte count as `du -h`-style human string.
fn format_size(bytes: u64) -> String {
    const K: u64 = 1024;
    const M: u64 = K * 1024;
    const G: u64 = M * 1024;
    const T: u64 = G * 1024;
    const P: u64 = T * 1024;

    if bytes >= P {
        format!("{:.1}P", bytes as f64 / P as f64)
    } else if bytes >= T {
        format!("{:.1}T", bytes as f64 / T as f64)
    } else if bytes >= G {
        format!("{:.1}G", bytes as f64 / G as f64)
    } else if bytes >= M {
        format!("{:.1}M", bytes as f64 / M as f64)
    } else if bytes >= K {
        format!("{:.1}K", bytes as f64 / K as f64)
    } else {
        format!("{bytes}B")
    }
}

pub fn compact(text: &str) -> String {
    if detect_df(text) {
        return compact_df(text);
    }
    if !detect_du(text) {
        return text.to_string();
    }

    // Lines that are NOT du entries are kept, not dropped. The detector fires
    // at 80 % du-shaped lines, so up to a fifth of the input could be something
    // else — and this compactor RE-SORTS by size, so anything it skipped lost
    // both its content and its position.
    //
    // Reported by two agents on 2026-08-03 and reproduced here: a script
    // printing `<count>  <path>` per file under `## section` headings is du-
    // shaped enough to fire, and came back as a size ranking with the section
    // headings gone. One of them needed the ORDER of the blocks and received
    // their WEIGHT; the other lost the headers of a scan. A `du: cannot read
    // directory …` error line disappeared the same way.
    //
    // Same rule as `netstat`, which keeps banners and column headers verbatim:
    // reformatting is allowed to reorder what it understands, never to silently
    // delete what it does not.
    let mut kept_verbatim: Vec<&str> = Vec::new();
    let mut entries: Vec<(u64, String, String)> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((size_b, path)) = parse_du_line(line) else {
            kept_verbatim.push(line);
            continue;
        };
        let raw_size = line.split_whitespace().next().unwrap_or("");
        entries.push((size_b, raw_size.to_string(), path.to_string()));
    }

    if entries.is_empty() {
        return text.to_string();
    }

    // Sort by size descending; ties broken alphabetically by path so the
    // output is deterministic across runs.
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));

    // Total:
    //   * If there's an explicit tree-root entry (path `.` or `/`), use its
    //     size — that's the cumulative `du` total.
    //   * Else if there's a `total` entry, use it.
    //   * Else for `du -sh *`-style multi-tree output, sum all entries.
    let tree_root = entries
        .iter()
        .find(|(_, _, p)| p == "." || p == "/" || p == "./" || p == "total")
        .map(|(s, _, _)| *s);
    let report_total = tree_root.unwrap_or_else(|| entries.iter().map(|(s, _, _)| *s).sum());
    let total_human = format_size(report_total);

    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "~{total_human} total");

    if !kept_verbatim.is_empty() {
        for line in &kept_verbatim {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("\nlargest entries:\n");
    for (_, size_raw, path) in entries.iter().take(TOP_ENTRIES_CAP) {
        let _ = writeln!(out, "  {size_raw:>8}  {path}");
    }
    if entries.len() > TOP_ENTRIES_CAP {
        let _ = writeln!(
            out,
            "  … {} more entr(y/ies) elided",
            entries.len() - TOP_ENTRIES_CAP
        );
    }

    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// L81 — `df` compactor. Drops snap loopbacks + empty tmpfs + other
/// noise mounts. Keeps real disks. Emits a one-line summary of what
/// was elided so the agent can ask for the raw output via expand_ref
/// when they specifically need to inspect the dropped rows.
pub fn compact_df(text: &str) -> String {
    let mut header: Option<&str> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    // Content BEFORE the df header (an earlier section of a mixed script
    // output) passes through verbatim — it used to be silently deleted.
    let mut preamble: Vec<&str> = Vec::new();
    for line in text.lines() {
        if header.is_none() && is_df_header(line) {
            header = Some(line);
            continue;
        }
        if header.is_some() {
            data_lines.push(line);
        } else {
            preamble.push(line);
        }
    }
    let Some(header_line) = header else {
        return text.to_string();
    };
    let mut kept: Vec<&str> = Vec::new();
    let mut snap_count = 0usize;
    let mut tmpfs_idle = 0usize;
    // Non-df lines after the header (`df -h && cargo test` in one output)
    // pass through instead of being reduced to an "N unparsed" count.
    let mut trailing: Vec<&str> = Vec::new();
    for line in &data_lines {
        let Some(row) = parse_df_line(line) else {
            // Trailing blank line or footer — keep silently.
            if !line.trim().is_empty() {
                trailing.push(line);
            }
            continue;
        };
        if is_df_noise(&row) {
            if row.filesystem.starts_with("/dev/loop") {
                snap_count += 1;
            } else if row.filesystem == "tmpfs" {
                tmpfs_idle += 1;
            }
            continue;
        }
        kept.push(line);
    }
    let mut elision_parts: Vec<String> = Vec::new();
    if snap_count > 0 {
        elision_parts.push(format!("{snap_count} snap loopback(s)"));
    }
    if tmpfs_idle > 0 {
        elision_parts.push(format!("{tmpfs_idle} idle tmpfs"));
    }
    // (unparsed lines are appended verbatim below, not elided)
    if elision_parts.is_empty() && trailing.is_empty() {
        // Nothing to drop — return verbatim so we don't strip useful
        // signal AND avoid claiming a win we didn't deliver.
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() / 2);
    for l in &preamble {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(header_line);
    out.push('\n');
    for k in &kept {
        out.push_str(k);
        out.push('\n');
    }
    if !elision_parts.is_empty() {
        let _ = writeln!(out, "  [elided: {}]", elision_parts.join(", "));
    }
    if !trailing.is_empty() {
        // Another command's output after the df block — keep it (one more
        // compaction pass may recognise its own format).
        out.push_str(&crate::auto::compact_remainder(&trailing.join("\n")));
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {

    /// Reported by two agents 2026-08-03 and reproduced: this compactor fires
    /// at 80 % du-shaped lines and then RE-SORTS by size, so whatever it did not
    /// recognise lost both its content and its position. A scan printing
    /// `<count>  <path>` under `## section` headings came back as a ranking with
    /// the headings gone — the agent needed the ORDER and received the WEIGHT.
    #[cfg(test)]
    mod keeps_what_it_does_not_understand {
        use super::super::compact;

        fn du_like(n: usize) -> String {
            (0..n)
                .map(|i| format!("{}\t./dir{i}\n", 4 * (i + 1)))
                .collect()
        }

        #[test]
        fn section_headings_survive_the_ranking() {
            let input = format!("## section A\n{}## section B\n{}", du_like(8), du_like(8));
            let out = compact(&input);
            assert!(
                out.contains("## section A") && out.contains("## section B"),
                "the headings were deleted: {out}"
            );
        }

        /// The case that matters most: `du` writes its failures to the same
        /// stream, and a deleted error reads as a clean run.
        #[test]
        fn a_du_error_line_is_never_dropped() {
            let input = format!(
                "{}du: cannot read directory './secret': Permission denied\n{}",
                du_like(8),
                du_like(8)
            );
            let out = compact(&input);
            assert!(
                out.contains("cannot read directory"),
                "the error vanished: {out}"
            );
        }

        /// And the compaction still happens — this is not a licence to stop
        /// compacting. Sized past the top-N cap so there is a tail to drop:
        /// below it the summary is legitimately longer than the input, which is
        /// why `try_compact` (not `compact`) carries the never-inflate guard.
        #[test]
        fn homogeneous_du_output_is_still_ranked() {
            let input: String = (0..400)
                .map(|i| format!("{}\t./a/rather/long/path/number/{i}/leaf\n", 4 * (i + 1)))
                .collect();
            let out = compact(&input);
            assert!(out.contains("largest entries:"), "no ranking: {out}");
            assert!(
                out.len() < input.len(),
                "no compaction: {} -> {}",
                input.len(),
                out.len()
            );
        }
    }
    use super::*;

    const DU_SAMPLE: &str = "4.0K\t./Cargo.toml
8.0K\t./README.md
24K\t./src
156M\t./vendor
1.2G\t./target/release
4.5G\t./target
12K\t./tests
4.0K\t./docs/api.md
8.0K\t./docs/architecture.md
12K\t./docs
4.5G\t.
";

    const DU_DEEP_SAMPLE: &str = "4.0K\t./.git/HEAD
12K\t./.git/objects/01
8.0K\t./.git/objects/02
4.0K\t./.git/refs/heads/main
4.0K\t./.git/refs
8.0K\t./.git/refs/heads
4.0K\t./.git/refs/tags
24K\t./.git/refs
156K\t./.git/objects
8.0K\t./.git/info
180K\t./.git
4.0K\t./src/main.rs
8.0K\t./src/lib.rs
12K\t./src
4.0K\t./Cargo.toml
200K\t.
";

    #[test]
    fn detects_du_output() {
        assert!(detect(DU_SAMPLE));
    }

    #[test]
    fn detects_deep_du() {
        assert!(detect(DU_DEEP_SAMPLE));
    }

    #[test]
    fn df_output_recognised_as_disk() {
        // Pre-L81 this test asserted `!detect(df)` — back then the
        // disk module only handled `du`. Now `df` is also detected
        // and routed through `compact_df`.
        let df = "Filesystem      Size  Used Avail Use% Mounted on\n/dev/sda1       100G   45G   55G  45% /\n/dev/sda2       500G  200G  300G  40% /home\ntmpfs           8.0G  1.5M  8.0G   1% /run\n";
        assert!(detect(df));
        // It's the `df` variant, not the `du` variant.
        assert!(detect_df(df));
        assert!(!detect_du(df));
    }

    #[test]
    fn rejects_prose() {
        assert!(!detect(
            "The system has 4G of RAM and 100G of disk. Some files are 12K each.\n"
        ));
    }

    #[test]
    fn rejects_code() {
        assert!(!detect(
            "fn main() {\n    const SIZE: usize = 4096;\n    println!(\"{SIZE}\");\n}\n"
        ));
    }

    #[test]
    fn extracts_entries_sorted_by_size() {
        let out = compact(DU_SAMPLE);
        /* L122: banner removed; assertion deleted */
        // Largest first.
        let largest_idx = out.find("4.5G").unwrap();
        let mid_idx = out.find("1.2G").unwrap();
        let smaller_idx = out.find("156M").unwrap();
        assert!(largest_idx < mid_idx);
        assert!(mid_idx < smaller_idx);
    }

    #[test]
    fn total_uses_tree_root() {
        let out = compact(DU_SAMPLE);
        // Tree root is `.` with 4.5G; same as the largest entry.
        assert!(out.contains("~4.5G total"));
    }

    #[test]
    fn handles_deep_tree() {
        let out = compact(DU_DEEP_SAMPLE);
        /* L122: banner removed; assertion deleted */
        // Top entries: 200K (.), 180K (.git), 156K (.git/objects)
        assert!(out.contains("200K"));
        assert!(out.contains("180K"));
        assert!(out.contains("156K"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not a du output";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn parse_size_handles_suffixes() {
        assert_eq!(parse_size("123"), Some(123));
        assert_eq!(parse_size("1K"), Some(1024));
        assert_eq!(parse_size("4.0K"), Some(4096));
        assert_eq!(parse_size("1.5M"), Some(1572864));
        assert_eq!(parse_size("2G"), Some(2_147_483_648));
        // Lower-case OK.
        assert_eq!(parse_size("4k"), Some(4096));
        // Non-size returns None.
        assert_eq!(parse_size("abc"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn format_size_round_trip() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(1023), "1023B");
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(1_572_864), "1.5M");
        assert_eq!(format_size(2_147_483_648), "2.0G");
        assert_eq!(format_size(1_099_511_627_776), "1.0T");
    }

    #[test]
    fn savings_huge_du() {
        // 2 000-entry synthetic du listing.
        let mut s = String::with_capacity(80_000);
        for d in 0..200 {
            for f in 0..10 {
                let size = match (d + f) % 4 {
                    0 => "4.0K",
                    1 => "12K",
                    2 => "1.5M",
                    _ => "8.0K",
                };
                let _ = writeln!(s, "{size}\t./dir{d}/sub/file_{f}");
            }
        }
        s.push_str("200M\t.\n");
        let (_out, sav) = compact_with_savings(&s);
        /* L122: banner removed; assertion deleted */
        assert!(sav.percent() > 95.0, "got {sav:?}");
    }

    #[test]
    fn df_detect_classic_header() {
        let s = "Filesystem      Size  Used Avail Use% Mounted on\n\
                 /dev/sda1       100G   50G   45G  53% /\n\
                 /dev/sda2       200G  100G   95G  52% /home\n\
                 tmpfs           8.0G  2.0G  6.0G  25% /run\n";
        assert!(detect_df(s));
        assert!(detect(s));
    }

    #[test]
    fn df_detect_french_locale() {
        // Real header from a French Linux: `Sys. de fichiers Taille
        // Utilisé Dispo Uti% Monté sur`. The detector must recognise
        // the locale-specific column names.
        let s = "Sys. de fichiers Taille Utilisé Dispo Uti% Monté sur\n\
                 /dev/sda1       100G   50G   45G  53% /\n\
                 /dev/sda2       200G  100G   95G  52% /home\n\
                 tmpfs           8.0G  2.0G  6.0G  25% /run\n";
        assert!(detect_df(s), "French df header missed");
    }

    #[test]
    fn df_compact_drops_snap_loopbacks() {
        let s = "Filesystem      Size  Used Avail Use% Mounted on\n\
                 /dev/loop0      166M  166M     0 100% /snap/firefox/3939\n\
                 /dev/loop1       56M   56M     0 100% /snap/core18/2979\n\
                 /dev/loop2       27M   27M     0 100% /var/lib/snapd/snap/dalfox/463\n\
                 /dev/loop12      49M   49M     0 100% /snap/snapd/26382\n\
                 /dev/nvme0n1p1   96M   32M   65M  34% /boot/efi\n\
                 tmpfs           8.0G  2.0G  6.0G  25% /run\n";
        let out = compact_df(s);
        assert!(
            out.contains("/dev/nvme0n1p1"),
            "real disk must survive: {out}"
        );
        assert!(out.contains("tmpfs"), "active tmpfs must survive: {out}");
        assert!(
            !out.contains("/dev/loop0"),
            "snap loopback must be elided: {out}"
        );
        assert!(
            out.contains("4 snap loopback"),
            "elision summary expected: {out}"
        );
    }

    #[test]
    fn df_compact_drops_idle_tmpfs() {
        let s = "Filesystem      Size  Used Avail Use% Mounted on\n\
                 /dev/sda1       100G   50G   45G  53% /\n\
                 tmpfs           8.0G     0  8.0G   0% /sys/fs/cgroup\n\
                 tmpfs           8.0G     0  8.0G   0% /run/user/0\n\
                 tmpfs           8.0G     0  8.0G   0% /run/user/1000\n";
        let out = compact_df(s);
        assert!(out.contains("/dev/sda1"));
        assert!(out.contains("idle tmpfs"));
    }

    #[test]
    fn df_compact_no_op_when_nothing_to_drop() {
        let s = "Filesystem      Size  Used Avail Use% Mounted on\n\
                 /dev/sda1       100G   50G   45G  53% /\n\
                 /dev/sda2       200G  100G   95G  52% /home\n\
                 /dev/sda3       300G  150G  145G  51% /var\n";
        let out = compact_df(s);
        // No noise → return verbatim so we don't pretend savings.
        assert_eq!(out, s);
    }

    #[test]
    fn df_with_dash_t_type_column() {
        // `df -hT` adds a Type column between Filesystem and Size.
        let s = "Filesystem     Type      Size  Used Avail Use% Mounted on\n\
                 /dev/sda1      ext4      100G   50G   45G  53% /\n\
                 /dev/sda2      ext4      200G  100G   95G  52% /home\n\
                 /dev/loop0     squashfs  166M  166M     0 100% /snap/firefox/3939\n\
                 /dev/loop1     squashfs   56M   56M     0 100% /snap/core18/2979\n\
                 /dev/loop2     squashfs   27M   27M     0 100% /snap/dalfox/463\n\
                 /dev/loop3     squashfs   49M   49M     0 100% /snap/snapd/26382\n";
        assert!(detect_df(s));
        let out = compact_df(s);
        assert!(out.contains("/dev/sda1"));
        assert!(out.contains("snap loopback"));
    }

    #[test]
    fn df_savings_real_shape() {
        // Realistic Ubuntu desktop df with 12 snap loopbacks + real
        // disks + tmpfs. Verify ≥ 50% savings.
        let mut s = String::from("Filesystem      Size  Used Avail Use% Mounted on\n");
        for i in 0..12 {
            let _ = writeln!(
                s,
                "/dev/loop{i:<2}     150M  150M     0 100% /snap/somesnap/{i}"
            );
        }
        s.push_str("/dev/nvme0n1p1   96M   32M   65M  34% /boot/efi\n");
        s.push_str("/dev/nvme0n1p5   1.7T  1.5T  101G  94% /\n");
        s.push_str("tmpfs            6.2G  104K  6.2G   1% /run/user/1000\n");
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("/dev/nvme0n1p5"));
        assert!(sav.percent() >= 50.0, "got {sav:?}\nout: {out}");
    }

    #[test]
    fn handles_unicode_paths() {
        let s = "4.0K\t./café/Müller.rs\n8.0K\t./café/déjà.rs\n12K\t./café\n4.0K\t./中文/数据.py\n8.0K\t./中文/测试.py\n12K\t./中文\n4.0K\t./🚀/launch.go\n8.0K\t./🚀/rocket.go\n12K\t./🚀\n36K\t.\n";
        assert!(detect(s));
        let out = compact(s);
        assert!(out.contains("café"));
        assert!(out.contains("中文"));
        assert!(out.contains('🚀'));
    }

    /// A table is not disk usage, however much its first column looks like a
    /// size and its second like a path.
    ///
    /// `gh pr list` in a pipe — which is how an agent always runs it — emits
    /// `<number>\t<title>\t<branch>\t<state>`. The branch holds a `/`, which
    /// waived the "a du path is one token" check, so 200 PR-shaped rows came
    /// back as `~214.9K total` / `largest entries:` — both false of the input
    /// — RE-SORTED descending by the PR number, which is the order a list
    /// exists to convey, with the tail elided. `du` separates size from path
    /// with one tab and its paths never contain another.
    #[test]
    fn a_tab_separated_table_is_not_disk_usage() {
        let mut t = String::new();
        t.push_str("1001\tthe first record of the list\tfeat/alpha\tKEEP-ME\n");
        for i in 2..=60 {
            t.push_str(&format!(
                "{}\trecord number {i} of the list\tfeat/br-{i}\t\n",
                1000 + i
            ));
        }
        assert!(
            !detect(&t),
            "a 4-column table must not be read as du:\n{}",
            compact(&t)
        );

        // Anti-vacuity, both halves. Real `du` still detects — including the
        // case that nearly broke the rule: a path with SPACES in it. Verified
        // against live `du -a` output, where no path carries a tab.
        let mut real = String::new();
        for i in 0..12 {
            real.push_str(&format!("{}K\t./src/mod{i}/file.rs\n", 4 * (i + 1)));
        }
        assert!(detect(&real), "real du must still be recognised");

        let mut spaced = String::new();
        for i in 0..12 {
            spaced.push_str(&format!(
                "{}K\t./mes documents/sous dossier {i}\n",
                4 * (i + 1)
            ));
        }
        assert!(
            detect(&spaced),
            "a du path may contain spaces — only a tab rules it out"
        );
    }
}
