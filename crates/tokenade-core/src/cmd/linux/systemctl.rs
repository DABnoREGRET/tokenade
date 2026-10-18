//! `systemctl` — list-units / status.
//!
//! Output shapes:
//!
//! - `systemctl list-units`:
//!   ```text
//!   UNIT                            LOAD   ACTIVE SUB     DESCRIPTION
//!   foo.service                     loaded active running ...
//!   bar.service                     loaded failed failed  ...
//!   ```
//! - `systemctl status <unit>`:
//!   ```text
//!   ● foo.service - Foo daemon
//!        Loaded: loaded (/lib/systemd/system/foo.service; enabled)
//!        Active: active (running) since ...
//!         Tasks: 5 (limit: ...)
//!        Memory: ...
//!        CGroup: /system.slice/foo.service
//!                ├─1234 /usr/bin/foo
//!                └─1235 /usr/bin/foo-worker
//!   ```
//!
//! Compaction:
//!   * list-units: drop inactive/dead units when there's no filter
//!     (`grep` is the right tool for that).
//!   * status: fold the cgroup process-tree to first 3 lines.
//!
//! Locale: status keywords (`active`, `failed`, `inactive`) are NOT
//! translated by systemd (verified across LANG=fr_FR.UTF-8 and
//! LANG=de_DE.UTF-8). Safe to match exact strings.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    let trimmed = text.trim_start();
    (trimmed.starts_with("UNIT") && trimmed.contains("LOAD") && trimmed.contains("ACTIVE"))
        || ((trimmed.starts_with('●') || trimmed.starts_with("* "))
            && (trimmed.contains("Loaded:") || trimmed.contains("Active:")))
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let trimmed = text.trim_start();
    if trimmed.starts_with("UNIT") {
        return compact_list_units(text);
    }
    compact_status(text)
}

fn compact_list_units(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let header = lines.first().copied().unwrap_or("");
    let mut kept: Vec<&str> = vec![header];
    let mut elided_inactive = 0usize;
    for line in lines.iter().skip(1) {
        let l = line.trim_end();
        if l.is_empty() {
            continue;
        }
        // Footer / summary trailing lines pass through.
        if l.starts_with("LOAD ") || l.starts_with("ACTIVE ") || l.starts_with("loaded units listed") {
            kept.push(line);
            continue;
        }
        // Heuristic: keep only failed/active/activating.
        if l.contains(" inactive ") || l.contains(" dead ") {
            elided_inactive += 1;
            continue;
        }
        kept.push(line);
    }
    let mut out = kept.join("\n");
    if elided_inactive > 0 {
        let _ = write!(
            out,
            "\n  [elided: {elided_inactive} inactive/dead unit(s) — re-run with --all to see them]"
        );
    }
    out
}

fn compact_status(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_cgroup = false;
    let mut cgroup_lines = 0usize;
    let mut total_cgroup_lines = 0usize;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("CGroup:") {
            in_cgroup = true;
            cgroup_lines = 0;
            total_cgroup_lines = 0;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_cgroup {
            let is_cgroup_child = t.starts_with('├')
                || t.starts_with('└')
                || t.starts_with('│')
                || (t.starts_with(char::is_numeric) && t.contains(' '));
            if !is_cgroup_child {
                if total_cgroup_lines > 3 {
                    let _ = writeln!(
                        out,
                        "                [{} more cgroup process(es) elided]",
                        total_cgroup_lines - 3
                    );
                }
                in_cgroup = false;
                out.push_str(line);
                out.push('\n');
                continue;
            }
            total_cgroup_lines += 1;
            if cgroup_lines < 3 {
                out.push_str(line);
                out.push('\n');
                cgroup_lines += 1;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // If we ended the input while still inside a cgroup block,
    // emit the elision summary now — otherwise the trailing
    // count is dropped on the floor.
    if in_cgroup && total_cgroup_lines > 3 {
        let _ = writeln!(
            out,
            "                [{} more cgroup process(es) elided]",
            total_cgroup_lines - 3
        );
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_list_units() {
        let s = "UNIT                            LOAD   ACTIVE SUB     DESCRIPTION\nfoo.service                     loaded active running Foo daemon\n";
        assert!(detect(s));
    }

    #[test]
    fn detects_status() {
        let s = "● foo.service - Foo daemon\n     Loaded: loaded (/lib/systemd/system/foo.service; enabled)\n     Active: active (running) since ...\n";
        assert!(detect(s));
    }

    #[test]
    fn drops_inactive_units_in_list() {
        let s = "UNIT                            LOAD   ACTIVE SUB     DESCRIPTION\nfoo.service                     loaded active running Foo daemon\nbar.service                     loaded inactive dead Bar daemon\nbaz.service                     loaded inactive dead Baz daemon\nqux.service                     loaded failed failed Qux daemon\n";
        let out = compact(s);
        assert!(out.contains("foo.service"));
        assert!(out.contains("qux.service"));
        assert!(!out.contains("bar.service"));
        assert!(out.contains("2 inactive"));
    }

    #[test]
    fn folds_long_cgroup_tree() {
        let mut s = String::from("● foo.service - Foo daemon\n     Loaded: loaded (/lib/systemd/system/foo.service; enabled)\n     Active: active (running)\n     CGroup: /system.slice/foo.service\n");
        for i in 0..20 {
            let _ = writeln!(s, "             ├─{}  /usr/bin/foo --worker={i}", 1000 + i);
        }
        let out = compact(&s);
        assert!(out.contains("CGroup:"));
        assert!(out.contains("cgroup process(es) elided"));
    }

    #[test]
    fn rejects_random() {
        assert!(!detect("hello world"));
    }
}
