//! `crontab -l` — user crontab listing.
//!
//! ```text
//! # m h  dom mon dow   command
//! # Example of job definition:
//! # .---------------- minute (0 - 59)
//! # |  .------------- hour (0 - 23)
//! # |  |  .---------- day of month (1 - 31)
//! # |  |  |  .------- month (1 - 12) OR jan,feb,mar,apr ...
//! # |  |  |  |  .---- day of week (0 - 6) (Sunday=0 or 7) OR sun,mon,tue,wed,thu,fri,sat
//! # |  |  |  |  |
//! # *  *  *  *  *  user-name  command to be executed
//! 0 3 * * * /usr/bin/backup.sh
//! 0 4 * * 1 /usr/bin/weekly-cleanup.sh
//! ```
//!
//! Compaction: drop the canonical comment block (the field reference
//! header). Keep user-edited comments + actual schedule lines.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    // Crontab has a clear shape: at least one line with 5 schedule
    // fields followed by a command. The most reliable signal is the
    // distinctive `# m h dom mon dow` header.
    let has_header = text.contains("# m h") || text.contains("# .---------------- minute");
    let has_schedule = text.lines().any(looks_like_schedule);
    has_header || has_schedule
}

fn looks_like_schedule(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() || t.starts_with('#') {
        return false;
    }
    let toks: Vec<&str> = t.split_whitespace().collect();
    if toks.len() < 6 {
        return false;
    }
    // First 5 tokens must be cron-shape (`*`, digit, comma-list,
    // dash-range, step) AND collectively contain at least one cron
    // syntactic marker (`*` or `/`) — without that guard, any 6+
    // word English sentence like "just some prose with no shape"
    // would qualify.
    let all_cron_shaped = toks.iter().take(5).all(|t| {
        t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '*' | ',' | '-' | '/' | '?'))
    });
    let has_cron_marker = toks
        .iter()
        .take(5)
        .any(|t| t.contains('*') || t.contains('/'));
    all_cron_shaped && has_cron_marker
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut drop_canonical_block = false;
    let mut elided_canonical = 0usize;
    for line in text.lines() {
        let t = line.trim_start();
        // Detect entering the canonical comment block.
        if t.starts_with("# Example of job definition")
            || t.starts_with("# m h  dom mon dow")
            || t.starts_with("# .---")
        {
            drop_canonical_block = true;
            elided_canonical += 1;
            continue;
        }
        if drop_canonical_block && t.starts_with("# ") && t.contains('|') {
            elided_canonical += 1;
            continue;
        }
        if drop_canonical_block && t.starts_with("# *") {
            elided_canonical += 1;
            continue;
        }
        // First non-canonical line ends the block.
        drop_canonical_block = false;
        out.push_str(line);
        out.push('\n');
    }
    if elided_canonical > 0 {
        let _ = writeln!(
            out,
            "  [elided: {elided_canonical} line(s) of canonical comment header]"
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

    const SAMPLE: &str = "# Edit this file to introduce tasks to be run by cron.\n#\n# m h  dom mon dow   command\n# Example of job definition:\n# .---------------- minute (0 - 59)\n# |  .------------- hour (0 - 23)\n# |  |  .---------- day of month (1 - 31)\n# |  |  |  .------- month (1 - 12) OR jan,feb,mar,apr ...\n# |  |  |  |  .---- day of week (0 - 6) (Sunday=0 or 7)\n# |  |  |  |  |\n# *  *  *  *  *  user-name  command to be executed\n0 3 * * * /usr/bin/backup.sh\n0 4 * * 1 /usr/bin/weekly-cleanup.sh\n";

    #[test]
    fn detects_crontab() {
        assert!(detect(SAMPLE));
    }

    #[test]
    fn drops_canonical_header() {
        let out = compact(SAMPLE);
        assert!(out.contains("/usr/bin/backup.sh"));
        assert!(!out.contains(".---------------- minute"));
        assert!(out.contains("canonical comment"));
    }

    #[test]
    fn rejects_random() {
        assert!(!detect("hello world"));
    }
}
