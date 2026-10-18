//! `pacman` — Arch / Manjaro package manager.
//!
//! Output shapes:
//!
//! - `pacman -S <pkg>` / `pacman -Syu`:
//!
//!   ```text
//!   :: Synchronizing package databases...
//!    core is up to date
//!    extra is up to date
//!    multilib is up to date
//!   :: Starting full system upgrade...
//!   resolving dependencies...
//!   looking for conflicting packages...
//!
//!   Packages (N) <pkg>-<ver> <pkg>-<ver> ...
//!
//!   Total Download Size:    123.45 MiB
//!   Total Installed Size:   543.21 MiB
//!   ```
//!
//! - `pacman -Q` (one line per installed package).
//! - `pacman -Qi <pkg>` (RFC 822-style key:value blocks).
//! - `pacman -F` (file ownership search).
//!
//! Compaction:
//!   * Q (list): fold to count + first 10 / last 5 packages.
//!   * Sync banners ("is up to date", "Synchronizing..."): keep.
//!   * Package install lists: keep verbatim — the agent needs them
//!     to verify what's being installed.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    // Quick markers — at least 2 of these strong signals.
    let mut score = 0;
    for line in text.lines().take(40) {
        if line.starts_with(":: Synchronizing") { score += 2; }
        if line.contains("is up to date") { score += 1; }
        if line.starts_with("Packages (") { score += 2; }
        if line.starts_with("Total Download Size:") { score += 2; }
        if line.starts_with("Total Installed Size:") { score += 2; }
        if line.starts_with("resolving dependencies...") { score += 1; }
        if line.starts_with(":: Proceed with installation?") { score += 2; }
    }
    score >= 2
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    // For `pacman -Q` style (one-package-per-line), fold to first 10
    // + count + last 5.
    if looks_like_pkg_list(text) {
        return fold_pkg_list(text);
    }
    // Otherwise: keep verbatim. The agent needs the dependency list.
    text.to_string()
}

fn looks_like_pkg_list(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 50 {
        return false;
    }
    // Each line should be `<name> <version>` shape (2 tokens, version
    // contains digit + `.` or `-`).
    let ok = lines
        .iter()
        .filter(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            toks.len() == 2
                && toks[1].chars().any(|c| c.is_ascii_digit())
                && (toks[1].contains('.') || toks[1].contains('-'))
        })
        .count();
    ok >= lines.len() * 8 / 10
}

fn fold_pkg_list(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let total = lines.len();
    let mut out = String::with_capacity(text.len() / 4);
    for line in lines.iter().take(10) {
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "  [... {} more packages elided ...]",
        total.saturating_sub(15)
    );
    for line in lines.iter().skip(total.saturating_sub(5)) {
        out.push_str(line);
        out.push('\n');
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
    fn detects_pacman_sync_output() {
        let s = ":: Synchronizing package databases...\n core is up to date\n extra is up to date\n";
        assert!(detect(s));
    }

    #[test]
    fn detects_pacman_install_summary() {
        let s = "resolving dependencies...\nPackages (3) foo-1.0 bar-2.0 baz-3.0\nTotal Download Size:    1.0 MiB\nTotal Installed Size:   2.0 MiB\n";
        assert!(detect(s));
    }

    #[test]
    fn folds_long_pacman_q_list() {
        let mut s = String::new();
        for i in 0..150 {
            let _ = writeln!(s, "pkg-{i} 1.0-{i}");
        }
        // Detector needs a strong pacman signal — `-Q` raw output
        // doesn't have one. We expose `compact` directly here to
        // test the folding logic regardless of the detector gate.
        assert!(looks_like_pkg_list(&s));
        let out = fold_pkg_list(&s);
        assert!(out.contains("pkg-0"));
        assert!(out.contains("pkg-149"));
        assert!(out.contains("[... 135 more packages elided ...]"));
    }

    #[test]
    fn keeps_short_outputs_verbatim() {
        let s = ":: Synchronizing package databases...\n core is up to date\n";
        let out = compact(s);
        assert_eq!(out, s);
    }
}
