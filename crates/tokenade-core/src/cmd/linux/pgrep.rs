//! `pgrep` (and the matched-PID echo of `pkill`) — list process IDs that
//! match a pattern.
//!
//! Output is one process per line, each beginning with a numeric PID:
//! bare `pgrep` prints just the PID, `pgrep -l` prints `PID name`, and
//! `pgrep -a` prints `PID full command line`. On a broad match (e.g.
//! `pgrep -a .` or matching a busy daemon) this runs to hundreds of
//! lines. The signal is *how many* matched plus a representative sample;
//! the noise is the long homogeneous middle.
//!
//! We fold the long run of PID lines into a count, keeping a head and tail
//! sample. Detection is structural — lines whose first token is an integer
//! PID — so it is locale-independent (there is no localised text here).


/// Consecutive PID lines before a run is worth folding.
const FOLD_THRESHOLD: usize = 30;
const KEEP_HEAD: usize = 10;
const KEEP_TAIL: usize = 5;

/// A process line: first whitespace-delimited token is an integer PID.
fn is_pid_line(line: &str) -> bool {
    match line.split_whitespace().next() {
        Some(tok) => !tok.is_empty() && tok.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

pub fn detect(text: &str) -> bool {
    text.lines().filter(|l| is_pid_line(l)).count() >= 3
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let total_pids = lines.iter().filter(|l| is_pid_line(l)).count();
    if total_pids <= FOLD_THRESHOLD {
        // Few matches — nothing worth folding; the central guard would
        // strip our banner anyway.
        return Some(crate::archetypes::passthrough(text));
    }

    Some(crate::archetypes::fold_runs(
        text,
        &lines,
        is_pid_line,
        FOLD_THRESHOLD,
        KEEP_HEAD,
        KEEP_TAIL,
        "more matching process(es) folded",
    ))
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use super::*;

    #[test]
    fn detector_rejects_empty_and_non_pid() {
        assert!(!detect(""));
        assert!(!detect("no such process\nnope\n"));
    }

    #[test]
    fn detector_accepts_pid_lines() {
        assert!(detect("1 /sbin/init\n2 [kthreadd]\n3 [pool]\n"));
    }

    #[test]
    fn folds_long_pgrep_a_listing() {
        // `pgrep -a .` style: PID + command line, hundreds of rows.
        let mut t = String::new();
        for pid in 100..200 {
            let _ = writeln!(t, "{pid} /usr/bin/worker --id {pid}");
        }
        let out = try_compact(&t).unwrap();
        assert!(out.contains("100 /usr/bin/worker"), "head bound kept");
        assert!(out.contains("199 /usr/bin/worker"), "tail bound kept");
        assert!(out.contains("matching process(es) folded"));
        assert!(!out.contains("150 /usr/bin/worker"), "middle folded");
        assert!(out.len() < t.len(), "must shrink");
    }

    #[test]
    fn bare_pid_column_folds() {
        let t: String = (1000..1100).map(|p| format!("{p}\n")).collect();
        let out = try_compact(&t).unwrap();
        assert!(out.contains("matching process(es) folded"));
    }

    #[test]
    fn short_match_passes_through() {
        // Detected (>=3 PID lines) but under the fold threshold.
        let out = try_compact("4242 /usr/bin/sshd\n12 /sbin/init\n7 [kthreadd]\n").unwrap();
        assert!(out.contains("4242 /usr/bin/sshd"));
        assert!(!out.contains("folded"));
    }
}
