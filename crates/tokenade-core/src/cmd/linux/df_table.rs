//! Shared engine for the `df` family (`df`, `df -h`, `-a`, `-i`, `-T`, `-x`).
//!
//! `df` prints a whitespace-aligned table whose semantically critical
//! column is the usage percentage: a near-full filesystem is the one
//! thing an agent debugging "no space left on device" actually needs to
//! see. We locate that column by *content* — tokens shaped like `NN%` —
//! rather than by header label, so the engine works under any locale.
//! Real coreutils output proves why label-matching is hopeless: under
//! `LANG=fr_FR` the header is `Sys. de fichiers … Uti%` (not the
//! `Système de fichiers`/`Util%` an English-centric guess assumes), and
//! German/Spanish differ again. Content detection sidesteps all of it.
//!
//! Rows at or above [`FULL_THRESHOLD`]% are surfaced first (the outcome
//! an agent cares about); the remainder are head-folded. The header line
//! is always kept verbatim. Nothing critical is ever dropped.

use std::fmt::Write;

const FULL_THRESHOLD: u32 = 90;

/// `87%` → `Some(87)`, `100%` → `Some(100)`, anything else → `None`.
fn percent_value(tok: &str) -> Option<u32> {
    let n = tok.strip_suffix('%')?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse::<u32>().ok()
}

/// Structural, locale-independent detector: a header line followed by at
/// least one data row carrying a percentage token.
pub fn detect(text: &str) -> bool {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    if lines.next().is_none() {
        return false;
    }
    lines.any(|l| l.split_whitespace().any(|t| percent_value(t).is_some()))
}

/// Fold a `df` table: header verbatim, then every filesystem at or above
/// [`FULL_THRESHOLD`]% usage, then the first `head` of the remainder with
/// a `(+N more)` summary. Returns `None` when the table is short enough
/// that there is nothing to fold (header + ≤ `head` rows).
pub fn fold(text: &str, head: usize, label: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() <= head + 1 {
        return None;
    }
    let header = lines[0];
    let rows = &lines[1..];
    let mut full: Vec<&str> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    for &r in rows {
        let pct = r.split_whitespace().filter_map(percent_value).max();
        if pct.is_some_and(|p| p >= FULL_THRESHOLD) {
            full.push(r);
        } else {
            rest.push(r);
        }
    }
    let total = rows.len();
    let mut out = String::with_capacity(text.len() / 2);
    if full.is_empty() {
        let _ = writeln!(out, "[tokenade:{label}] {total} filesystem(s)");
    } else {
        let _ = writeln!(
            out,
            "[tokenade:{label}] {total} filesystem(s), {} \u{2265}{FULL_THRESHOLD}% full first",
            full.len()
        );
    }
    out.push_str(header);
    out.push('\n');
    for r in &full {
        out.push_str(r);
        out.push('\n');
    }
    let shown = rest.len().min(head);
    for r in rest.iter().take(shown) {
        out.push_str(r);
        out.push('\n');
    }
    if rest.len() > shown {
        let _ = writeln!(out, "  ... (+{} more) ...", rest.len() - shown);
    }
    Some(out.trim_end().to_string())
}

// --- Shared-helper stubs ---------------------------------------------------
// This module is an engine shared by the `df` family, not a command of its
// own, but `build.rs` auto-declares every non-`mod.rs` file as a dispatch
// entry and expects the compactor quartet. Mirror `docker_table`: expose
// inert stubs so a stray `df_table` dispatch is a no-op. The real entry
// points are [`detect`] and [`fold`], called by the per-flag modules.

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
    fn percent_parsing() {
        assert_eq!(percent_value("87%"), Some(87));
        assert_eq!(percent_value("100%"), Some(100));
        assert_eq!(percent_value("0%"), Some(0));
        assert_eq!(percent_value("%"), None);
        assert_eq!(percent_value("87"), None);
        assert_eq!(percent_value("ab%"), None);
    }

    #[test]
    fn detects_french_header_by_content() {
        // Real `df -h` under LANG=fr_FR — label-matching would miss it.
        let fr = "Sys. de fichiers Taille Utilisé Dispo Uti% Monté sur\n\
                  /dev/sda1 100G 50G 50G 50% /\n\
                  tmpfs 31G 0 31G 0% /run\n";
        assert!(detect(fr));
    }

    #[test]
    fn surfaces_full_filesystems_first() {
        let mut t = String::from("Filesystem Size Used Avail Use% Mounted on\n");
        // 40 healthy loop devices, then one nearly-full real disk buried.
        for i in 0..40 {
            let _ = writeln!(t, "/dev/loop{i} 100M 1M 99M 1% /snap/x{i}");
        }
        t.push_str("/dev/sdb1 500G 480G 20G 96% /data\n");
        let out = fold(&t, 30, "df-h").unwrap();
        // The full disk is surfaced near the top, not lost in the tail.
        let full_pos = out.find("/data").unwrap();
        let fold_pos = out.find("more) ...").unwrap();
        assert!(full_pos < fold_pos, "full FS must precede the fold:\n{out}");
        assert!(out.contains("1 \u{2265}90% full first"));
        assert!(out.contains("Filesystem"));
    }

    #[test]
    fn short_table_passes_through() {
        let t = "Filesystem Size Used Avail Use% Mounted on\n/dev/sda1 100G 50G 50G 50% /\n";
        assert!(fold(t, 30, "df").is_none());
    }
}
