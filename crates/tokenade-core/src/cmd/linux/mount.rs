//! `mount` (without args, list current mounts).
//!
//! ```text
//! sysfs on /sys type sysfs (rw,nosuid,nodev,noexec,relatime)
//! proc on /proc type proc (rw,nosuid,nodev,noexec,relatime)
//! /dev/nvme0n1p1 on / type ext4 (rw,relatime)
//! /dev/loop0 on /snap/firefox/3939 type squashfs (ro,nodev,relatime)
//! tmpfs on /run/user/1000 type tmpfs (rw,nosuid,nodev,relatime,size=...)
//! cgroup on /sys/fs/cgroup/cpu type cgroup (rw,nosuid,nodev,noexec,relatime,cpu)
//! ```
//!
//! Compaction:
//!   * Drop snap squashfs (`/dev/loopN` mounted under `/snap/`).
//!   * Drop cgroup* mounts.
//!   * Drop proc / sysfs / devpts / securityfs / debugfs / tracefs.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

crate::archetypes::detect_gated_try_compact!();

const NOISE_FS: &[&str] = &[
    "proc", "sysfs", "devpts", "securityfs", "debugfs", "tracefs", "cgroup",
    "cgroup2", "fusectl", "configfs", "pstore", "bpf", "mqueue", "hugetlbfs",
    "ramfs", "binfmt_misc", "autofs",
];

pub fn compact(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut elided_snap = 0usize;
    let mut elided_cgroup_proc = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Parse: `<src> on <dst> type <fs> (<opts>)`
        let Some((src_part, rest)) = line.split_once(" on ") else {
            kept.push(line);
            continue;
        };
        let Some((dst, type_part)) = rest.split_once(" type ") else {
            kept.push(line);
            continue;
        };
        let fs_kind = type_part.split_whitespace().next().unwrap_or("");
        let src = src_part.trim();
        let dst = dst.trim();
        // Snap squashfs noise.
        if src.starts_with("/dev/loop") && (dst.starts_with("/snap/") || dst.contains("/snapd/snap/")) {
            elided_snap += 1;
            continue;
        }
        // Pseudo-fs noise.
        if NOISE_FS.contains(&fs_kind) {
            elided_cgroup_proc += 1;
            continue;
        }
        kept.push(line);
    }
    let mut out = String::with_capacity(text.len());
    for line in &kept {
        out.push_str(&fold_option_path_lists(line));
        out.push('\n');
    }
    if elided_snap > 0 || elided_cgroup_proc > 0 {
        let mut parts: Vec<String> = Vec::new();
        if elided_snap > 0 {
            parts.push(format!("{elided_snap} snap squashfs"));
        }
        if elided_cgroup_proc > 0 {
            parts.push(format!("{elided_cgroup_proc} cgroup/proc/sysfs etc."));
        }
        let _ = writeln!(out, "  [elided: {}]", parts.join(", "));
    }
    out.trim_end().to_string()
}


/// **The root of a path list, stated once per option.**
///
/// A Docker `overlay` line carries `lowerdir=` with eight to twelve layers,
/// all under `/var/lib/docker/overlay2/l`. MEASURED 2026-09-09 on the corpus
/// capture: four lines of this kind weigh **3 492 bytes**, of which **918
/// are that root rewritten at each layer**. Nothing disappears — every layer
/// name is still there, in order — and the rule is that of
/// [`crate::filelist::collapse_path_list`], shared with file
/// listing.
fn fold_option_path_lists(line: &str) -> String {
    let Some(open) = line.rfind(" (") else {
        return line.to_string();
    };
    if !line.ends_with(')') {
        return line.to_string();
    }
    let (head, opts) = line.split_at(open + 2);
    let opts = &opts[..opts.len() - 1];
    let mut folded = String::with_capacity(opts.len());
    let mut changed = false;
    for (i, opt) in opts.split(',').enumerate() {
        if i > 0 {
            folded.push(',');
        }
        match opt.split_once('=') {
            Some((key, value)) => match crate::filelist::collapse_path_list(value) {
                Some(short) => {
                    changed = true;
                    let _ = write!(folded, "{key}={short}");
                }
                None => folded.push_str(opt),
            },
            None => folded.push_str(opt),
        }
    }
    if !changed {
        return line.to_string();
    }
    format!("{head}{folded})")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"sysfs on /sys type sysfs (rw,nosuid,nodev,noexec,relatime)
proc on /proc type proc (rw,nosuid,nodev,noexec,relatime)
/dev/nvme0n1p1 on / type ext4 (rw,relatime)
/dev/loop0 on /snap/firefox/3939 type squashfs (ro,nodev,relatime)
/dev/loop1 on /snap/core18/2979 type squashfs (ro,nodev,relatime)
/dev/loop2 on /snap/snapd/26382 type squashfs (ro,nodev,relatime)
cgroup on /sys/fs/cgroup/cpu type cgroup (rw,cpu)
tmpfs on /run/user/1000 type tmpfs (rw,size=10M)
"#;

    /// **The root of a path list is stated once.**
    ///
    /// Fixture: the real `overlay` line from the corpus capture, the one that
    /// carries twelve layers. The test first checks that the case HAPPENS
    /// (the line is indeed folded), then that nothing disappeared — the twelve
    /// layer names are still there, in order.
    #[test]
    fn an_option_path_list_states_its_root_once() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/linux/mount_full.txt"),
        )
        .expect("the corpus `mount` capture");
        let line = body
            .lines()
            .filter(|l| l.contains("lowerdir="))
            .max_by_key(|l| l.len())
            .expect("an overlay line");
        let folded = super::fold_option_path_lists(line);
        assert!(
            folded.len() + 200 < line.len(),
            "the line barely moved: {} -> {}",
            line.len(),
            folded.len()
        );
        assert!(folded.contains("lowerdir=/var/lib/docker/overlay2/l/{"));
        // Every layer name survives.
        let names: Vec<&str> = line
            .split("lowerdir=")
            .nth(1)
            .and_then(|v| v.split(&[',', ')'][..]).next())
            .expect("the lowerdir value")
            .split(':')
            .map(|p| p.rsplit('/').next().unwrap_or(p))
            .collect();
        assert_eq!(names.len(), 12, "the fixture no longer has twelve layers");
        for n in names {
            assert!(folded.contains(n), "layer {n} disappeared");
        }
    }

    #[test]
    fn detects_mount_output() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn drops_snap_and_pseudo_fs() {
        let out = compact(SAMPLE);
        assert!(out.contains("/dev/nvme0n1p1"));
        assert!(out.contains("tmpfs"));
        assert!(!out.contains("/snap/firefox/3939"));
        assert!(!out.contains("type proc"));
        assert!(!out.contains("type sysfs"));
        assert!(out.contains("3 snap squashfs"));
    }

    #[test]
    fn rejects_random() {
        // Detector is now permissive (routes are gated by program
        // name through `dispatch_named`, not content shape). Only
        // empty input is rejected.
        assert!(!detect(""));
        assert!(!detect("   \n  "));
    }

    #[test]
    fn savings_meaningful() {
        let (_o, sav) = compact_with_savings(SAMPLE);
        assert!(sav.percent() > 30.0, "{sav:?}");
    }
}
