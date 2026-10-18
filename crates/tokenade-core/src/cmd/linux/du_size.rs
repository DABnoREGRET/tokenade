//! Shared engine for the `du` family (`du`, `du -h`, `-a`, `-d N`, `du -sh *`).
//!
//! `du` prints one `SIZE<TAB>PATH` line per directory in post-order, so a
//! deep tree produces hundreds of lines whose only actionable content is
//! "which paths are big". We parse the size column — human units
//! (`1.3M`, `324K`, base-1024) or raw block counts, with a `,` or `.`
//! decimal so it works under any locale — and surface the largest entries
//! first, head-folding the small ones. Sorting by size also guarantees the
//! root total (du's largest, last-printed line) is never decapitated.
//!
//! Detection is structural: most lines are `<size> <path>`. No header to
//! localise.

use std::fmt::Write;

/// Parse a `du` size field: optional decimal (`,`/`.`) number with an
/// optional `K/M/G/T/P` unit (case-insensitive, optional `i`/`B` suffix),
/// returned in bytes for comparison. Bare numbers (raw block counts) keep
/// their own scale, which is consistent within one `du` run.
fn parse_size(tok: &str) -> Option<f64> {
    let t = tok.trim();
    if t.is_empty() {
        return None;
    }
    let bytes = t.as_bytes();
    // Split the leading numeric part (digits, one decimal separator).
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_sep = false;
    for &b in bytes {
        if b.is_ascii_digit() {
            seen_digit = true;
            end += 1;
        } else if (b == b'.' || b == b',') && !seen_sep {
            seen_sep = true;
            end += 1;
        } else {
            break;
        }
    }
    if !seen_digit {
        return None;
    }
    let num: f64 = t[..end].replace(',', ".").parse().ok()?;
    let unit = t[end..].trim();
    let mult = match unit.chars().next().map(|c| c.to_ascii_uppercase()) {
        None => 1.0,
        Some('K') => 1024.0,
        Some('M') => 1024.0 * 1024.0,
        Some('G') => 1024.0 * 1024.0 * 1024.0,
        Some('T') => 1024.0_f64.powi(4),
        Some('P') => 1024.0_f64.powi(5),
        Some('B') => 1.0,
        _ => return None, // not a size token
    };
    Some(num * mult)
}

/// A line is a `du` entry when its first field parses as a size and a path
/// follows.
fn entry_size(line: &str) -> Option<f64> {
    let mut it = line.split_whitespace();
    let sz = parse_size(it.next()?)?;
    it.next()?; // a path must follow
    Some(sz)
}

pub fn detect(text: &str) -> bool {
    let mut entries = 0usize;
    let mut total = 0usize;
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        total += 1;
        if entry_size(l).is_some() {
            entries += 1;
        }
    }
    total > 0 && entries * 2 >= total // majority are size+path lines
}

/// Fold a `du` listing: keep the `head` largest entries (size desc), drop
/// the small tail to a count. Returns `None` when short enough to leave be.
pub fn fold(text: &str, head: usize, label: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() <= head + 1 {
        return None;
    }
    // (size, line) for sortable entries; non-entries kept aside verbatim.
    let mut sized: Vec<(f64, &str)> = Vec::new();
    let mut other: Vec<&str> = Vec::new();
    for &l in &lines {
        match entry_size(l) {
            Some(s) => sized.push((s, l)),
            None => other.push(l),
        }
    }
    if sized.len() <= head {
        return None;
    }
    sized.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let total = sized.len();
    let shown = head.min(total);
    let mut out = String::with_capacity(text.len() / 2);
    let _ = writeln!(
        out,
        "[tokenade:{label}] {total} path(s), {shown} largest first"
    );
    for (_, l) in sized.iter().take(shown) {
        out.push_str(l);
        out.push('\n');
    }
    if total > shown {
        let _ = writeln!(out, "  ... (+{} smaller) ...", total - shown);
    }
    for l in &other {
        out.push_str(l);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

// --- Shared-helper stubs (see df_table.rs for the rationale) ---------------
pub fn try_compact(_text: &str) -> Option<String> {
    None
}
pub fn compact(text: &str) -> String {
    crate::archetypes::passthrough(text)
}
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_human_and_locale_sizes() {
        assert_eq!(parse_size("324K"), Some(324.0 * 1024.0));
        assert_eq!(parse_size("1.3M"), Some(1.3 * 1024.0 * 1024.0));
        assert_eq!(parse_size("1,3M"), Some(1.3 * 1024.0 * 1024.0)); // FR comma
        assert_eq!(parse_size("88M"), Some(88.0 * 1024.0 * 1024.0));
        assert_eq!(parse_size("4096"), Some(4096.0)); // raw blocks
        assert_eq!(parse_size("path"), None);
    }

    #[test]
    fn detects_du_listing() {
        assert!(detect("1,3M\tcrates/cli\n86M\tcrates/core\n88M\tcrates\n"));
        assert!(!detect("error: permission denied\nanother prose line\n"));
    }

    #[test]
    fn surfaces_largest_first_keeps_root_total() {
        let mut t = String::new();
        for i in 0..60 {
            let _ = writeln!(t, "4,0K\tcrates/core/src/cmd/x{i}");
        }
        // Big intermediate dirs and the root total (du prints these last).
        t.push_str("86M\tcrates/core\n88M\tcrates\n");
        let out = fold(&t, 30, "du-h").unwrap();
        let first_line = out.lines().nth(1).unwrap();
        assert!(first_line.contains("88M"), "largest must be first:\n{out}");
        // Root total survives (it's the biggest, never decapitated).
        assert!(out.contains("88M\tcrates"));
        assert!(out.contains("smaller) ..."));
    }

    #[test]
    fn short_listing_passes_through() {
        assert!(fold("88M\tcrates\n", 30, "du-sh").is_none());
    }
}
