//! Windows `dir` — directory listing.
//!
//! ```text
//!  Volume in drive C is OS
//!  Volume Serial Number is 1234-ABCD
//!
//!  Directory of C:\Users\me
//!
//! 2026-05-29  09:00    <DIR>          .
//! 2026-05-29  09:00    <DIR>          ..
//! 2026-05-28  17:42         1,234,567 some_file.txt
//! 2026-05-27  16:00    <DIR>          Documents
//!     12 File(s)      9,876,543 bytes
//!      4 Dir(s)  100,000,000,000 bytes free
//! ```
//!
//! Compaction: every entry, its padding collapsed, while the listing fits
//! the delivery threshold; past it, the first 30 entries + summary footer.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    // A real fr-FR `dir` (2026-10-05) says `633 fichier(s)` in lower case, and
    // reaches a UTF-8 reader in the console code page: `R‚pertoire de`. Neither
    // matched, and the listing went to the stride-sampler (`in 94 runs of 5`).
    let lower = text.to_ascii_lowercase();
    lower.contains("directory of") || lower.contains("pertoire de")
        || lower.contains("file(s)") || lower.contains("fichier(s)")
        || text.contains("<DIR>") || text.contains("<REP>")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut entry_lines: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // Entry rows start with a date `YYYY-MM-DD` or `MM/DD/YYYY`.
        let t = line.trim_start();
        if t.len() >= 10 {
            let bytes = t.as_bytes();
            let date_dash = bytes[4] == b'-' && bytes[7] == b'-';
            let date_slash = bytes[2] == b'/' && bytes[5] == b'/';
            if date_dash || date_slash {
                entry_lines.push(i);
            }
        }
    }
    if entry_lines.len() <= 30 {
        return text.to_string();
    }
    // **A listing that fits keeps every name.** The cut below kept the first
    // thirty entries of an alphabetical listing: measured 2026-10-02 on a real
    // `dir C:\Windows` (fr-FR, 103 entries, 5 KB — under the delivery
    // threshold), everything after `HelpPane.exe` was gone, `System32` and
    // `WinSxS` among it. In a listing the names ARE the answer (same finding
    // as `ls -l`, which keeps every row). What fits is served whole, its
    // column padding collapsed; only what does not fit is cut.
    let entries: std::collections::HashSet<usize> = entry_lines.iter().copied().collect();
    let collapsed: String = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if entries.contains(&i) {
                collapse_columns(l)
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if tk_base::delivery::fits(&collapsed) {
        return collapsed;
    }
    let first_entry = entry_lines[0];
    let last_entry = *entry_lines.last().unwrap();
    let elision_target = entry_lines[30];
    let mut out = String::with_capacity(text.len() / 2);
    for line in lines.iter().take(elision_target) {
        out.push_str(line);
        out.push('\n');
    }
    let _ = first_entry;
    let _ = last_entry;
    let _ = writeln!(
        out,
        "  [... {} more entries elided ...]",
        entry_lines.len() - 30
    );
    // Always keep the footer (2 lines after last entry).
    for line in lines.iter().skip(last_entry + 1).take(3) {
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// `2026-05-29  09:00    <DIR>          my  dir` → `2026-05-29 09:00 <DIR> my  dir`.
/// Only the columns BEFORE the name are re-spaced: a name may itself hold a
/// run of spaces, and re-spacing it would deliver a file that does not exist.
/// The name starts after the size or `<DIR>`-like column; a line where that
/// column is not found is kept as it was.
fn collapse_columns(line: &str) -> String {
    let mut rest = line;
    let mut head: Vec<&str> = Vec::new();
    while let Some(start) = rest.find(|c: char| !c.is_whitespace()) {
        let tail = &rest[start..];
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let field = &tail[..end];
        head.push(field);
        rest = &tail[end..];
        let is_size = field.starts_with('<') && field.ends_with('>')
            || field.chars().all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '\u{a0}' | '\u{202f}'));
        if head.len() >= 3 && is_size {
            let name = rest.trim_start();
            if name.is_empty() {
                break;
            }
            return format!("{} {name}", head.join(" "));
        }
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_english_dir() {
        let s = " Directory of C:\\Users\\me\n2026-05-29  09:00    <DIR>          .\n";
        assert!(detect(s));
    }

    #[test]
    fn detects_french_repertoire() {
        let s = " Répertoire de C:\\Utilisateurs\\moi\n2026-05-29  09:00    <REP>          .\n";
        assert!(detect(s));
    }

    #[test]
    fn folds_long_listing() {
        let mut s = String::from(" Directory of C:\\Users\\me\n");
        for i in 0..50 {
            let _ = writeln!(
                s,
                "2026-05-29  09:00         {:>10} file_{i}.txt",
                i * 100
            );
        }
        s.push_str("    50 File(s)      9,876,543 bytes\n");
        let out = compact(&s);
        assert!(!out.contains("more entries elided"), "{out}");
        for i in 0..50 {
            assert!(out.contains(&format!(" file_{i}.txt\n")), "file_{i}: {out}");
        }
        assert!(out.contains("2026-05-29 09:00 4900 file_49.txt"), "{out}");
        assert!(out.contains("File(s)"));
    }

    #[test]
    fn a_name_keeps_its_own_spaces() {
        assert_eq!(
            collapse_columns("19/08/2026  06:35    <DIR>          my  old   dir"),
            "19/08/2026 06:35 <DIR> my  old   dir"
        );
        assert_eq!(
            collapse_columns("05/29/2026  09:00 AM         1,234 a  b.txt"),
            "05/29/2026 09:00 AM 1,234 a  b.txt"
        );
    }

    #[test]
    fn cuts_only_a_listing_past_the_threshold() {
        let mut s = String::from(" Directory of C:\\Windows\\WinSxS\n");
        for i in 0..6000 {
            let _ = writeln!(s, "2026-05-29  09:00    <DIR>          amd64_component_{i}");
        }
        s.push_str("    6000 Dir(s)      9,876,543 bytes free\n");
        let out = compact(&s);
        assert!(out.contains("more entries elided"), "{}", &out[..200]);
        assert!(out.contains("Dir(s)"));
    }
}

#[cfg(test)]
mod fr_fr_console {
    /// A real fr-FR `dir C:\Windows\System32\*.exe`, read as UTF-8 from the
    /// console code page: recognised, never stride-sampled, footer kept.
    #[test]
    fn a_french_dir_in_the_console_code_page_is_a_dir() {
        let text = String::from_utf8_lossy(include_bytes!("../../../tests/fixtures/linux/windows_dir_frfr_oem.txt")).into_owned();
        assert!(super::detect(&text));
        let out = super::compact(&text);
        assert!(!out.contains("runs of"), "{out}");
        assert!(out.contains("633 fichier(s)"), "{out}");
        assert!(out.len() < text.len());
    }
}
