//! `ls -l` / `ls -la` / `ls -R` — long-format directory listing.
//!
//! The signal in a long listing is the *non-regular* entries — every
//! directory, symlink, device, socket and named pipe — plus the `total`
//! header and (in `ls -R`) the per-directory `path:` section headers. The
//! noise is a directory holding hundreds of plain regular files: a blind
//! head-slice would decapitate the bottom (the last entries, and any
//! trailing directory of a recursive listing).
//!
//! We fold long *runs of consecutive regular-file entries* into a count,
//! keeping a few at each end so the alphabetical bounds survive, and keep
//! every directory/symlink/device entry, the `total` line and section
//! headers verbatim.
//!
//! Detection is structural and locale-independent: the 10-char Unix mode
//! block (`-rw-r--r--`, `drwxr-xr-x`, `lrwxrwxrwx`, …) is emitted
//! identically in every locale, so nothing here matches translated text
//! like `total` / `gesamt` / `insgesamt`.


/// Consecutive regular-file entries before a run is worth folding.
const FOLD_THRESHOLD: usize = 24;
/// How many entries to keep at the head of a folded run.
const KEEP_HEAD: usize = 8;
/// How many entries to keep at the tail of a folded run.
const KEEP_TAIL: usize = 4;

/// The leading file-type characters of a Unix long-listing mode block.
const TYPE_CHARS: &[char] = &['-', 'd', 'l', 'b', 'c', 's', 'p'];

/// Is `line` a long-format `ls -l` entry? Matches the 10-char mode block
/// at the start: one type char followed by nine permission chars drawn
/// from `rwxsStT-` (and the GNU `+`/`.`/`@` ACL/attr suffix is tolerated
/// because we only inspect the first ten bytes).
fn mode_type(line: &str) -> Option<char> {
    let mut chars = line.chars();
    let t = chars.next()?;
    if !TYPE_CHARS.contains(&t) {
        return None;
    }
    let perms: String = chars.by_ref().take(9).collect();
    if perms.chars().count() == 9
        && perms
            .chars()
            .all(|c| matches!(c, 'r' | 'w' | 'x' | 's' | 'S' | 't' | 'T' | '-'))
    {
        Some(t)
    } else {
        None
    }
}

fn is_entry(line: &str) -> bool {
    mode_type(line).is_some()
}

/// A plain regular file (`-`) — the foldable noise. Directories, symlinks
/// and devices are signal and always kept.
fn is_regular_file(line: &str) -> bool {
    mode_type(line) == Some('-')
}

pub fn detect(text: &str) -> bool {
    // At least a few real long-listing entries — not a stray line that
    // merely begins with a dash.
    text.lines().filter(|l| is_entry(l)).count() >= 3
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let total_regular = lines.iter().filter(|l| is_regular_file(l)).count();
    if total_regular < FOLD_THRESHOLD {
        // Small listing — nothing worth folding; the central guard would
        // strip our banner anyway.
        return Some(crate::archetypes::passthrough(text));
    }

    Some(crate::archetypes::fold_runs(
        text,
        &lines,
        is_regular_file,
        FOLD_THRESHOLD,
        KEEP_HEAD,
        KEEP_TAIL,
        "regular file(s) folded",
    ))
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use super::*;

    fn big_dir(n_files: usize) -> String {
        let mut t = String::from("total 1234\n");
        t.push_str("drwxr-xr-x 2 u u 4096 Jun 17 10:00 .\n");
        t.push_str("drwxr-xr-x 4 u u 4096 Jun 17 10:00 ..\n");
        t.push_str("drwxr-xr-x 2 u u 4096 Jun 17 10:00 subdir\n");
        for i in 0..n_files {
            let _ = writeln!(t, "-rw-r--r-- 1 u u 0 Jun 17 10:00 file_{i:04}.txt");
        }
        t.push_str("lrwxrwxrwx 1 u u 7 Jun 17 10:00 link -> file_0000.txt\n");
        t
    }

    #[test]
    fn detector_rejects_empty_and_plain() {
        assert!(!detect(""));
        assert!(!detect("just\nsome\nplain text\n"));
    }

    #[test]
    fn detector_accepts_long_listing() {
        assert!(detect(&big_dir(30)));
    }

    #[test]
    fn folds_regular_files_keeps_dirs_links_and_header() {
        let t = big_dir(100);
        let out = try_compact(&t).unwrap();
        // Header and non-regular entries always survive.
        assert!(out.contains("total 1234"), "total header kept");
        assert!(out.contains("drwxr-xr-x 2 u u 4096 Jun 17 10:00 subdir"), "dir kept");
        assert!(out.contains("link -> file_0000.txt"), "symlink kept");
        // Bounds of the folded run survive.
        assert!(out.contains("file_0000.txt"), "first file kept");
        assert!(out.contains("file_0099.txt"), "last file kept");
        // Middle folded into a count.
        assert!(out.contains("regular file(s) folded"));
        assert!(!out.contains("file_0050.txt"), "middle folded away");
        assert!(out.len() < t.len(), "must shrink");
    }

    #[test]
    fn short_listing_passes_through() {
        let t = big_dir(5);
        let out = try_compact(&t).unwrap();
        assert!(out.contains("file_0004.txt"));
        assert!(!out.contains("folded"));
    }

    #[test]
    fn locale_independent_no_total_word_needed() {
        // German `ls` prints `insgesamt` instead of `total`; detection
        // must still fire purely on the mode blocks.
        let mut t = String::from("insgesamt 1234\n");
        for i in 0..40 {
            let _ = writeln!(t, "-rw-r--r-- 1 u u 0 Jun 17 10:00 datei_{i:02}.txt");
        }
        let out = try_compact(&t).unwrap();
        assert!(out.contains("regular file(s) folded"));
    }
}
