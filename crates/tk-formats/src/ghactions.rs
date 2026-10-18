//! GitHub Actions workflow-log compaction.
//!
//! `gh run view --log` and the raw log file download produce streams that
//! look like:
//!
//! ```text
//! 2024-01-15T10:30:00.1234567Z ##[group]Set up job
//! 2024-01-15T10:30:00.1234568Z Current runner version: '2.310.0'
//! 2024-01-15T10:30:00.1234569Z ##[group]Operating System
//! 2024-01-15T10:30:00.1234570Z   Ubuntu
//! 2024-01-15T10:30:00.1234571Z   22.04.3
//! 2024-01-15T10:30:00.1234572Z   LTS
//! 2024-01-15T10:30:00.1234573Z ##[endgroup]
//! 2024-01-15T10:30:01.1234567Z ##[group]Run actions/checkout@v4
//! …thousands more timestamped lines…
//! 2024-01-15T10:39:55.1234567Z ##[error]Process completed with exit code 1.
//! 2024-01-15T10:39:55.1234568Z ::error::file=src/main.py,line=42::ImportError
//! ```
//!
//! Every line is prefixed with a 28-byte ISO-8601 nanosecond timestamp; on
//! a 50 000-line log that's 1.4 MB of clock-noise the agent doesn't need
//! (the time is monotonic anyway). 90 %+ of the remaining bytes are routine
//! step chatter — `Successfully installed`, dependency resolution lists,
//! `actions/checkout@v4 …` headers, etc.
//!
//! What the agent reading the log actually wants:
//!   - the **outline**: list of `##[group]` headers (= step names)
//!   - every `##[error]` and `::error::` annotation (the diagnostic gold)
//!   - every `##[warning]` and `::warning::` annotation
//!   - every `::notice::` annotation
//!   - the final exit-code line if present
//!
//! Real-world ratio: ≥95 % on production CI logs.

use std::fmt::Write;

pub const GROUPS_CAP: usize = 200;
pub const ERRORS_CAP: usize = 200;
pub const WARNINGS_CAP: usize = 100;
pub const NOTICES_CAP: usize = 50;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);

    // Strong signals: GHA workflow directives. A REAL GHA log has
    // dozens of these markers; documentation that merely mentions
    // them (CHANGELOG describing the compactor, stealth.rs doc-
    // comments quoting the format) has 1-2. Require ≥2 hits across
    // the recognised marker set.
    let marker_hits = [
        "##[group]",
        "##[endgroup]",
        "##[error]",
        "##[warning]",
        "##[section]",
        "##[command]",
        "##[debug]",
    ]
    .iter()
    .map(|m| head.matches(m).count())
    .sum::<usize>();
    if marker_hits >= 2 {
        return true;
    }

    // Workflow command annotations (`::error::`, `::warning::`, `::notice::`,
    // `::group::`) appear at start of line (possibly after a timestamp or
    // ANSI colour codes).
    // Strong markers (##[…] directives and ::workflow-cmd::) are unique enough
    // to be reliable. We refuse to guess GHA from timestamps alone — false
    // positives are worse than missed detection on logs that lack them.
    head.lines().any(|l| {
        let no_ts = strip_iso_timestamp(l);
        let no_ansi = strip_ansi(no_ts);
        let t = no_ansi.trim_start();
        t.starts_with("::error")
            || t.starts_with("::warning")
            || t.starts_with("::notice")
            || t.starts_with("::group::")
            || t.starts_with("::endgroup::")
    })
}

/// True if `line` starts with a valid ISO-8601 datetime ending in `Z`
/// (with or without fractional-second component).
fn has_iso_timestamp(line: &str) -> bool {
    let bytes = line.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    // YYYY-MM-DDTHH:MM:SS prefix shape.
    if !(bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':')
    {
        return false;
    }
    // Position 19 is either Z (no fraction) or . (fraction starts).
    bytes[19] == b'Z' || bytes[19] == b'.'
}

/// Strip ANSI CSI escape sequences (e.g. `\x1b[36m`, `\x1b[0m`) so the
/// downstream prefix-match step works even on colourised runner output.
/// Returns the borrowed input unchanged when no ESC byte is present (the
/// common case in raw `gh run view --log` downloads).
fn strip_ansi(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('\x1b') {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ … <ascii-letter>. Drop the whole sequence.
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&cc) = chars.peek() {
                    chars.next();
                    if cc.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            // Lone ESC (or other forms) — silently dropped.
        } else {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Strip the ISO-8601 timestamp prefix (and its trailing space) if present.
/// Handles both `…SSZ ` and `…SS.NNNZ ` variants, with any fractional width.
fn strip_iso_timestamp(line: &str) -> &str {
    if !has_iso_timestamp(line) {
        return line;
    }
    let bytes = line.as_bytes();
    let z_idx = if bytes[19] == b'Z' {
        19
    } else {
        // Fraction → find Z afterward.
        match line[20..].find('Z') {
            Some(rel) => rel + 20,
            None => return line,
        }
    };
    // Skip Z and an optional trailing space.
    let after_z = z_idx + 1;
    if line.len() > after_z && line.as_bytes()[after_z] == b' ' {
        &line[after_z + 1..]
    } else {
        &line[after_z..]
    }
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }

    let mut groups: Vec<(String, usize)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut notices: Vec<String> = Vec::new();
    let mut commands_traced: Vec<String> = Vec::new();
    let mut conclusion: Option<String> = None;
    // Total counts include items beyond the per-bucket cap; the *vec* lengths
    // are the displayed counts. The header reports totals so the user can see
    // how much was elided.
    let mut total_groups = 0usize;
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;
    let mut total_notices = 0usize;
    for raw in text.lines() {
        let no_ts = strip_iso_timestamp(raw);
        let no_ansi = strip_ansi(no_ts);
        let line = no_ansi.trim_end();
        let trimmed = line.trim_start();

        // Group transitions — `##[group]` and `::group::` are both valid.
        if let Some(name) = trimmed.strip_prefix("##[group]") {
            total_groups += 1;
            groups.push((name.to_string(), 0));
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("::group::") {
            total_groups += 1;
            groups.push((name.to_string(), 0));
            continue;
        }
        if trimmed.starts_with("##[endgroup]") || trimmed.starts_with("::endgroup::") {
            continue;
        }

        // Errors.
        if let Some(rest) = trimmed.strip_prefix("##[error]") {
            total_errors += 1;
            if let Some(g) = groups.last_mut() {
                g.1 += 1;
            }
            if errors.len() < ERRORS_CAP {
                errors.push(rest.to_string());
            }
            // Some `##[error]Process completed with exit code N` lines are
            // also the conclusion marker.
            if rest.starts_with("Process completed with exit code") {
                conclusion = Some(rest.to_string());
            }
            continue;
        }
        if trimmed.starts_with("::error") {
            total_errors += 1;
            if let Some(g) = groups.last_mut() {
                g.1 += 1;
            }
            if errors.len() < ERRORS_CAP {
                errors.push(trimmed.to_string());
            }
            continue;
        }

        // Warnings.
        if let Some(rest) = trimmed.strip_prefix("##[warning]") {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(rest.to_string());
            }
            continue;
        }
        if trimmed.starts_with("::warning") {
            total_warnings += 1;
            if warnings.len() < WARNINGS_CAP {
                warnings.push(trimmed.to_string());
            }
            continue;
        }

        // Notices.
        if trimmed.starts_with("::notice") {
            total_notices += 1;
            if notices.len() < NOTICES_CAP {
                notices.push(trimmed.to_string());
            }
            continue;
        }

        // `##[command]` is the shell command being executed by an action.
        // Keep them — they're often diagnostic.
        if let Some(rest) = trimmed.strip_prefix("##[command]") {
            if commands_traced.len() < 50 {
                commands_traced.push(rest.to_string());
            }
            continue;
        }

        // `##[section]Starting:` / `##[section]Finishing:` mark job stage
        // transitions. Not in groups but useful — fold into commands list.
        if let Some(rest) = trimmed.strip_prefix("##[section]") {
            if commands_traced.len() < 50 {
                commands_traced.push(format!("[section] {rest}"));
            }
            continue;
        }

        // `##[debug]` is for workflow authors' debugging; drop.
    }

    // ---- Render ----
    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "{total_groups} groups, {total_errors} error(s), {total_warnings} warning(s), {total_notices} notice(s)"
    );
    if let Some(c) = &conclusion {
        let _ = writeln!(out, "conclusion: {c}");
    }

    if !groups.is_empty() {
        out.push_str("\ngroups:\n");
        // A group's output FOLLOWS its `endgroup`, so an error belongs to the
        // last group opened. That group is the step that failed: it is named
        // with its count, and the cap applies to the clean groups only — the
        // failing step past the 200th was elided with the rest.
        let mut clean_shown = 0usize;
        for (g, errs) in &groups {
            if *errs > 0 {
                let _ = writeln!(out, "  {g}  ✗ {errs} error(s)");
            } else if clean_shown < GROUPS_CAP {
                clean_shown += 1;
                let _ = writeln!(out, "  {g}");
            }
        }
        let shown = clean_shown + groups.iter().filter(|(_, e)| *e > 0).count();
        if total_groups > shown {
            let _ = writeln!(out, "  … {} more group(s) elided", total_groups - shown);
        }
    }

    if !commands_traced.is_empty() {
        let _ = write!(out, "\ncommands & sections ({}):\n", commands_traced.len());
        for c in &commands_traced {
            out.push_str("  ");
            out.push_str(c);
            out.push('\n');
        }
    }

    crate::archetypes::capped_section(&mut out, "errors", &errors, total_errors, "error(s)");

    crate::archetypes::capped_section(
        &mut out,
        "warnings",
        &warnings,
        total_warnings,
        "warning(s)",
    );

    crate::archetypes::capped_section(&mut out, "notices", &notices, total_notices, "notice(s)");

    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "2024-01-15T10:30:00.1234567Z ##[group]Set up job\n2024-01-15T10:30:00.1234568Z Current runner version: '2.310.0'\n2024-01-15T10:30:00.1234569Z ##[group]Operating System\n2024-01-15T10:30:00.1234570Z   Ubuntu\n2024-01-15T10:30:00.1234571Z   22.04.3\n2024-01-15T10:30:00.1234572Z   LTS\n2024-01-15T10:30:00.1234573Z ##[endgroup]\n2024-01-15T10:30:01.1234567Z ##[group]Run actions/checkout@v4\n2024-01-15T10:30:01.1234568Z ##[command]/usr/bin/git config --global --add safe.directory /repo\n2024-01-15T10:30:01.1234569Z Initialized empty Git repository\n2024-01-15T10:30:01.1234570Z ##[endgroup]\n2024-01-15T10:30:05.1234567Z ##[group]Run pytest\n2024-01-15T10:30:05.1234568Z ##[warning]deprecated_action=actions/checkout@v3\n2024-01-15T10:30:08.1234567Z ::error::file=src/main.py,line=42::ImportError: cannot import module\n2024-01-15T10:30:08.1234568Z ::warning::file=src/utils.py,line=10::variable shadows builtin\n2024-01-15T10:30:08.1234569Z ::notice::Running on ubuntu-latest\n2024-01-15T10:30:08.1234570Z ##[error]Process completed with exit code 1.\n2024-01-15T10:30:08.1234571Z ##[endgroup]\n2024-01-15T10:30:09.1234567Z ##[section]Finishing: Run pytest\n";

    #[test]
    fn the_failing_step_is_named_even_past_the_group_cap() {
        let mut s = String::new();
        for i in 0..(GROUPS_CAP + 20) {
            let _ = writeln!(s, "2024-01-01T00:00:00.0000000Z ##[group]Run step {i}");
            s.push_str("2024-01-01T00:00:00.0000000Z ##[endgroup]\n");
            s.push_str("2024-01-01T00:00:00.0000000Z some output\n");
            if i == GROUPS_CAP + 10 {
                s.push_str(
                    "2024-01-01T00:00:00.0000000Z ##[error]Process completed with exit code 1.\n",
                );
            }
        }
        let out = compact(&s);
        assert!(
            out.contains(&format!("  Run step {}  ✗ 1 error(s)", GROUPS_CAP + 10)),
            "{out}"
        );
        assert!(out.contains("  Run step 0\n"), "{out}");
        assert!(
            !out.contains(&format!("Run step {}\n", GROUPS_CAP + 11)),
            "{out}"
        );
        assert!(out.contains("… 19 more group(s) elided"), "{out}");
    }
    #[test]
    fn detects_with_group_marker() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn detects_with_only_workflow_cmd() {
        let s = "::error::file=foo.py,line=1::oops\n::warning::other\n::notice::FYI\n".to_string();
        assert!(detect(&s));
    }

    #[test]
    fn rejects_plain_timestamped_lines_without_gha_markers() {
        // ISO timestamps alone are NOT GHA — every structured app
        // logger emits them. Routing such output through the GHA
        // compactor produced empty `0 groups, 0 errors` summaries
        // and lost the real content. Detector must refuse.
        let mut s = String::new();
        for i in 0..15 {
            let _ = writeln!(s, "2024-01-15T10:30:{i:02}.1234567Z some text");
        }
        assert!(!detect(&s));
    }

    #[test]
    fn rejects_plain_logs() {
        let s = "INFO server started\nINFO listening on 8080\nWARNING something fishy\n";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_rust_code() {
        assert!(!detect("fn main() {\n    println!(\"hello\");\n}\n"));
    }

    #[test]
    fn extracts_groups_errors_warnings() {
        let out = compact(SAMPLE);
        /* L122: banner removed; assertion deleted */
        // Conclusion picked up.
        assert!(out.contains("conclusion: Process completed with exit code 1"));
        // Group names extracted.
        assert!(out.contains("Set up job"));
        assert!(out.contains("Operating System"));
        assert!(out.contains("Run actions/checkout@v4"));
        assert!(out.contains("Run pytest"));
        // Errors verbatim (both ## and :: shapes).
        assert!(out.contains("Process completed with exit code 1"));
        assert!(out.contains("::error::file=src/main.py,line=42::ImportError"));
        // Warnings.
        assert!(out.contains("deprecated_action=actions/checkout@v3"));
        assert!(out.contains("::warning::file=src/utils.py,line=10"));
        // Notices.
        assert!(out.contains("::notice::Running on ubuntu-latest"));
        // Routine chatter must NOT leak.
        assert!(!out.contains("Current runner version"));
        assert!(!out.contains("22.04.3"));
        assert!(!out.contains("Initialized empty Git repository"));
        // No raw timestamps in output.
        assert!(!out.contains("2024-01-15T"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "this is not a GHA log";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn strip_timestamp_nano() {
        let line = "2024-01-15T10:30:00.1234567Z hello world";
        assert_eq!(strip_iso_timestamp(line), "hello world");
    }

    #[test]
    fn strip_timestamp_seconds_only() {
        let line = "2024-01-15T10:30:00Z hello";
        assert_eq!(strip_iso_timestamp(line), "hello");
    }

    #[test]
    fn strip_timestamp_milliseconds() {
        let line = "2024-01-15T10:30:00.123Z hello";
        assert_eq!(strip_iso_timestamp(line), "hello");
    }

    #[test]
    fn strip_timestamp_noop_on_plain() {
        let line = "no timestamp here";
        assert_eq!(strip_iso_timestamp(line), "no timestamp here");
    }

    #[test]
    fn savings_huge_log() {
        // 30 000-line synthetic CI log: groups + chatter + a few real errors.
        let mut s = String::with_capacity(2_000_000);
        for i in 0..3_000 {
            let _ = writeln!(s, "2024-01-15T10:30:00.{i:07}Z ##[group]Step {i}");
            for j in 0..8 {
                let _ = writeln!(
                    s,
                    "2024-01-15T10:30:00.{:07}Z routine output line {j} for step {i}",
                    i * 100 + j
                );
            }
            s.push_str("2024-01-15T10:30:01.0000000Z ##[endgroup]\n");
        }
        s.push_str("2024-01-15T10:30:02.0000000Z ##[error]Process completed with exit code 1.\n");
        s.push_str("2024-01-15T10:30:02.0000001Z ::error::Something went wrong\n");
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("3000 groups"));
        assert!(out.contains("Process completed with exit code 1"));
        assert!(sav.percent() > 95.0, "got {sav:?}");
    }
}
