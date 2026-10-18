//! A content detector must not claim a payload it does not own.
//!
//! Routing by CONTENT is a first-match-wins ladder of 44 predicates, so a
//! greedy one shadows the correct one further down and nothing says so. The
//! failure is worse than a poor fold: the wrong compactor renders its own
//! vocabulary over someone else's bytes, and the result STATES THINGS THAT ARE
//! NOT TRUE of the input.
//!
//! Three times measured, three times the same shape:
//!
//! * a base64 blob read as a directory listing — "plain-listing — 3509
//!   entries … grouped by directory" (2026-09-04);
//! * a unified diff folded by a head+tail slice instead of the diff parser,
//!   under a banner counting the INPUT (2026-09-04);
//! * a tab-separated table of PR records read as disk usage — a total size and
//!   a "largest entries" heading, rows re-sorted largest-first, tail dropped
//!   (2026-09-06). It cost compaction too: 4,063 bytes of false disk usage
//!   where the rightful route answers in 468.
//!
//! Each case below is MEASURED, not imagined, and each carries its
//! anti-vacuity twin: the rightful owner must still claim its own canonical
//! sample, so the matrix cannot be satisfied by a detector that refuses
//! everything.

use tokenade_core::auto::{detect, Kind};

/// A `gh pr list` in a pipe — how an agent always runs it: no header, a
/// numeric first column, a branch column carrying a `/`.
fn pr_table() -> String {
    let mut t = String::from("1001\tthe first record of the list\tfeat/alpha\tKEEP-ME\n");
    for i in 2..=60 {
        t.push_str(&format!(
            "{}\trecord number {i} of the list\tfeat/br-{i}\t\n",
            1000 + i
        ));
    }
    t
}

/// Real `du -h` output, including the case that nearly made a narrower rule
/// wrong: directories whose names contain SPACES.
fn real_du() -> String {
    let mut t = String::new();
    for i in 0..14 {
        t.push_str(&format!("{}K\t./src/mod{i}/file.rs\n", 4 * (i + 1)));
    }
    t.push_str("64K\t./mes documents/sous dossier\n");
    t.push_str("128K\t.\n");
    t
}

/// A base64 payload. `/` is one of its 64 symbols, which is what made it read
/// as a path listing.
fn base64_blob() -> String {
    let mut t = String::new();
    let alphabet: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .chars()
        .collect();
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    for _ in 0..400 {
        for _ in 0..76 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            t.push(alphabet[(x % 64) as usize]);
        }
        t.push('\n');
    }
    t
}

/// A real `ls -la`-shaped listing: the rightful owner of `FileList`.
fn real_listing() -> String {
    let mut t = String::from("total 480\n");
    for i in 0..40 {
        t.push_str(&format!(
            "-rw-r--r--  1 user group  {:>6} Sep  6 01:0{} src/module{i}/file{i}.rs\n",
            1024 * (i + 1),
            i % 10
        ));
    }
    t
}

fn unified_diff() -> String {
    let mut t = String::from("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,40 +1,40 @@\n");
    for i in 0..60 {
        t.push_str(&format!("-    let old_value_{i} = compute({i});\n"));
        t.push_str(&format!("+    let new_value_{i} = compute({i} + 1);\n"));
    }
    t
}

/// The matrix. A row is a payload, the kinds that must NOT claim it, and — so
/// the guard cannot pass by refusing everything — the kind that must.
#[test]
fn a_measured_shape_is_claimed_only_by_its_owner() {
    struct Case {
        what: &'static str,
        text: String,
        never: &'static [Kind],
    }
    let cases = [
        Case {
            what: "a tab-separated table of PR records (gh pr list in a pipe)",
            text: pr_table(),
            never: &[Kind::Disk, Kind::FileList, Kind::Markdown],
        },
        Case {
            what: "a base64 blob",
            text: base64_blob(),
            never: &[Kind::FileList, Kind::Disk, Kind::Csv, Kind::Tsv],
        },
        Case {
            what: "a unified diff",
            text: unified_diff(),
            never: &[Kind::Disk, Kind::FileList, Kind::Logs],
        },
    ];
    for c in &cases {
        let got = detect(&c.text);
        assert!(
            !c.never.contains(&got),
            "{} was claimed by {got:?}, which does not own it",
            c.what
        );
    }

    // Anti-vacuity: every detector named above still claims what IS its own.
    assert_eq!(detect(&real_du()), Kind::Disk, "real du must still be Disk");
    assert_eq!(
        detect(&real_listing()),
        Kind::FileList,
        "a real listing must still be FileList"
    );
    assert_eq!(
        detect(&unified_diff()),
        Kind::UnifiedDiff,
        "a diff must reach the diff parser, not a generic slice"
    );
}
