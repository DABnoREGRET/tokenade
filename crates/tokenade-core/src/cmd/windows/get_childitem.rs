//! `Get-ChildItem -Recurse` — one table per directory, read as the paths it
//! lists.
//!
//! ```text
//!     Répertoire : C:\Windows\System32\drivers
//!
//! Mode                 LastWriteTime         Length Name
//! ----                 -------------         ------ ----
//! d-----        30/05/2026     13:06                de-DE
//! -a----        07/12/2019     10:07         266240 1394ohci.sys
//! ```
//!
//! A recursive listing went to the generic table spec: its first 50 rows,
//! then `+1264 more` — measured 2026-10-05 on a real `Get-ChildItem -Recurse
//! C:\Windows\System32\drivers` (fr-FR, 157 810 bytes, 22 directories), the
//! agent saw the first directory and nothing of the other 21. Each row is
//! turned into its full path (directory + name, a folder ending in `\`) and
//! the list goes through the same path-listing fold as `find`, which groups
//! by directory. A listing of one directory is left to the table spec.

/// `Directory: C:\x` / `Répertoire : C:\x` (any locale, any decoding): a
/// section header is an indented `<word> : <drive path>` line.
fn section_dir(line: &str) -> Option<&str> {
    let t = line.trim();
    let (_, path) = t.split_once(':')?;
    let path = path.trim_start_matches(|c: char| c == '\u{a0}' || c.is_whitespace() || c == '\u{fffd}');
    crate::filelist::is_windows_path(path).then_some(path)
}

pub fn detect(text: &str) -> bool {
    text.lines().filter(|l| l.starts_with(' ') && section_dir(l).is_some()).count() >= 2
        && text.lines().any(|l| l.trim_start().starts_with("Mode "))
}

crate::archetypes::detect_gated_try_compact!();

/// `d-----  30/05/2026  13:06        de-DE` → (`de-DE`, true);
/// `-a----  07/12/2019  10:07  266240 1394ohci.sys` → (`1394ohci.sys`, false).
fn row(line: &str) -> Option<(&str, bool)> {
    let mode = line.split_whitespace().next()?;
    if mode.len() != 6 || !mode.chars().all(|c| "dalrhs-".contains(c)) {
        return None;
    }
    let dir = mode.starts_with('d');
    // Mode, date, time, then the length for a file: the name is what follows.
    let skip = if dir { 3 } else { 4 };
    let mut rest = line.trim_start();
    for _ in 0..skip {
        rest = rest.split_once(char::is_whitespace)?.1.trim_start();
    }
    (!rest.is_empty()).then_some((rest.trim_end(), dir))
}

pub fn compact(text: &str) -> String {
    // A folder listed in its parent AND as a section of its own would be said
    // twice; only an empty (or unreadable) one needs its parent row.
    let sections: std::collections::HashSet<String> = text
        .lines()
        .filter(|l| l.starts_with(' '))
        .filter_map(section_dir)
        .map(|d| d.trim_end_matches('\\').to_ascii_lowercase())
        .collect();
    let mut paths: Vec<String> = Vec::new();
    let mut current: Option<&str> = None;
    for line in text.lines() {
        if let Some(d) = section_dir(line).filter(|_| line.starts_with(' ')) {
            current = Some(d);
            continue;
        }
        let (Some(dir), Some((name, is_dir))) = (current, row(line)) else {
            continue;
        };
        let path = format!("{}\\{name}", dir.trim_end_matches('\\'));
        if is_dir && sections.contains(&path.to_ascii_lowercase()) {
            continue;
        }
        paths.push(if is_dir { path + "\\" } else { path });
    }
    if paths.len() < 10 {
        return text.to_string();
    }
    let out = crate::filelist::compact(&paths.join("\n"));
    if out.len() >= text.len() {
        return text.to_string();
    }
    out
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recursive_listing_is_folded_by_directory() {
        let text = "\n    Directory: C:\\proj\n\nMode                 LastWriteTime         Length Name\n----                 -------------         ------ ----\nd-----        30/05/2026     13:06                src\n-a----        07/12/2019     10:07           2662 Cargo.toml\n\n    Directory: C:\\proj\\src\n\nMode                 LastWriteTime         Length Name\n----                 -------------         ------ ----\n"
            .to_string()
            + &(0..40).map(|i| format!("-a----        07/12/2019     10:07          {i:>5} mod{i}.rs\n")).collect::<String>();
        assert!(detect(&text));
        let out = compact(&text);
        assert!(out.contains("C:\\proj"), "{out}");
        assert!(!out.contains("runs of"), "{out}");
        assert!(out.len() < text.len(), "{out}");
        // One directory alone is the table spec's.
        assert!(!detect("\n    Directory: C:\\proj\n\nMode   LastWriteTime   Length Name\n"));
    }

    #[test]
    fn a_row_gives_its_name_whatever_the_spaces() {
        assert_eq!(row("d-----        30/05/2026     13:06                de-DE"), Some(("de-DE", true)));
        assert_eq!(row("-a----        07/12/2019     10:07         266240 my file.sys"), Some(("my file.sys", false)));
        assert_eq!(row("Mode                 LastWriteTime         Length Name"), None);
    }
}

#[cfg(test)]
mod real_capture {
    use super::*;

    /// A real `Get-ChildItem -Recurse C:\Windows\System32\drivers` on a fr-FR
    /// console, read through the OEM code page (`Répertoire` arrives mangled),
    /// 18 lines kept per directory: every directory must still be named.
    #[test]
    fn every_directory_of_a_recursive_listing_is_named() {
        let raw = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/windows/get_childitem_recurse_frfr.txt")).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(detect(&text));
        let out = compact(&text);
        eprintln!("{out}");
        assert!(out.contains("C:\\Windows\\System32\\drivers"), "{out}");
        for sub in ["de-DE\\", "etc\\hosts", "UMDF\\uk-UA\\", "wd\\WdBoot.sys", "DriverData\\"] {
            assert!(out.contains(sub), "{sub} missing:\n{out}");
        }
        assert_eq!(out.lines().filter(|l| *l == "UMDF\\de-DE\\").count(), 1, "{out}");
        assert!(out.len() * 3 < text.len(), "{} → {}", text.len(), out.len());
    }
}
