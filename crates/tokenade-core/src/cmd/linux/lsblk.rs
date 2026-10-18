//! `lsblk` — block device tree.
//!
//! ```text
//! NAME          MAJ:MIN RM   SIZE RO TYPE MOUNTPOINTS
//! loop0           7:0    0 245.5M  1 loop /snap/firefox/3939
//! loop1           7:1    0  55.4M  1 loop /snap/core18/2979
//! ...
//! nvme0n1       259:0    0 953.9G  0 disk
//! ├─nvme0n1p1   259:1    0    96M  0 part /boot/efi
//! ├─nvme0n1p2   259:2    0   512M  0 part /boot
//! └─nvme0n1p3   259:3    0 953.3G  0 part /
//! ```
//!
//! Compaction: drop snap loopback (`loop*` mounted under `/snap/`).

use std::fmt::Write;

/// Column names that identify an `lsblk` header, whichever set was selected.
///
/// The check used to require `SIZE` **and** `TYPE`, which names the DEFAULT
/// column set and nothing else. `lsblk -f` prints
/// `NAME FSTYPE FSVER LABEL UUID FSAVAIL FSUSE% MOUNTPOINTS` — no `SIZE`, and
/// `TYPE` matched only because `FSTYPE` contains it — so the filesystem view
/// went untouched: measured 2026-09-04 on this machine, 33 lines of which
/// **15 are snap loopbacks**, the exact rows this module exists to drop.
/// `-o`, `-m` and `-t` select other sets again.
///
/// Two of these are required rather than one, because `detect` also runs in
/// the content-shape ladder where the program name is unknown: `NAME` plus a
/// single common word would let any two-column table stop the ladder here.
const LSBLK_COLUMNS: &[&str] = &["SIZE", "TYPE", "MOUNTPOINT", "MAJ:MIN", "UUID", "RM "];

pub fn detect(text: &str) -> bool {
    let mut first = "";
    for l in text.lines() {
        if !l.trim().is_empty() {
            first = l;
            break;
        }
    }
    let header_ok = first.trim_start().starts_with("NAME")
        && LSBLK_COLUMNS.iter().filter(|c| first.contains(**c)).count() >= 2;
    let data_lines = text.lines().skip(1).take(80).filter(|l| !l.trim().is_empty()).count();
    header_ok && data_lines >= 3
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut elided = 0usize;
    for line in text.lines() {
        if line.contains("/snap/") && line.contains("loop") {
            elided += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if elided > 0 {
        let _ = writeln!(out, "  [elided: {elided} snap loop(s)]");
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "NAME          MAJ:MIN RM   SIZE RO TYPE MOUNTPOINTS\nloop0           7:0    0 245.5M  1 loop /snap/firefox/3939\nloop1           7:1    0  55.4M  1 loop /snap/core18/2979\nloop2           7:2    0  91.7M  1 loop /snap/gtk-common-themes/1535\nnvme0n1       259:0    0 953.9G  0 disk\n├─nvme0n1p1   259:1    0    96M  0 part /boot/efi\n└─nvme0n1p3   259:3    0 953.3G  0 part /\n";

    #[test]
    fn detects_lsblk() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn drops_snap_loops() {
        let out = compact(SAMPLE);
        assert!(out.contains("nvme0n1"));
        assert!(!out.contains("/snap/firefox"));
        assert!(out.contains("3 snap loop"));
    }

    #[test]
    fn rejects_random() {
        assert!(!detect("just text"));
    }

    /// `lsblk -f` — the filesystem view. Different columns, same rows to drop.
    const FS_SAMPLE: &str = concat!(
        "NAME        FSTYPE   FSVER LABEL UUID                                 FSAVAIL FSUSE% MOUNTPOINTS\n",
        "loop0       squashfs 4.0                                                    0   100% /var/lib/snapd/snap/core22/2411\n",
        "loop1       squashfs 4.0                                                    0   100% /var/lib/snapd/snap/core20/2769\n",
        "loop2       squashfs 4.0                                                    0   100% /var/lib/snapd/snap/core18/2999\n",
        "nvme0n1                                                                                \n",
        "|-nvme0n1p1 vfat     FAT32       1234-ABCD                             80.1M    16% /boot/efi\n",
        "`-nvme0n1p3 ext4     1.0   root  00000000-1111-2222-3333-444444444444  120.5G    45% /\n",
    );

    /// The header names the columns the invocation asked for, and every set is
    /// still `lsblk`. `-f` carries no `SIZE` at all.
    #[test]
    fn every_column_set_is_still_lsblk() {
        assert!(detect(FS_SAMPLE), "the -f header must be recognised");
        assert!(!FS_SAMPLE.lines().next().unwrap().contains("SIZE"));
        assert!(detect(SAMPLE), "the default set must keep working");

        let out = compact(FS_SAMPLE);
        assert!(out.contains("nvme0n1p3"), "the real devices stay");
        assert!(!out.contains("core22"), "the snap loopbacks go");
        assert!(out.contains("3 snap loop"));
    }

    /// `detect` also runs in the content-shape ladder, where the program name
    /// is unknown. A table that merely starts with NAME must not stop it.
    #[test]
    fn a_table_that_is_not_lsblk_does_not_stop_the_ladder() {
        let two_col = "NAME      SIZE\nalpha     10\nbeta      20\ngamma     30\n";
        assert!(
            !detect(two_col),
            "NAME plus one common column is any table, not lsblk"
        );

        let elsewhere = "TOOL NAME   SIZE   TYPE\na 1 x\nb 2 y\nc 3 z\n";
        assert!(
            !detect(elsewhere),
            "the header must BEGIN with NAME, not merely mention it"
        );
    }
}
