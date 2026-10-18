//! `chmod -v` / `chmod -c` / `chmod -R -v` — verbose permission changes.
//!
//! With `-v` (or `-c`) chmod prints one line per file describing the mode
//! it set, e.g. `mode of 'x' changed from 0644 (rw-r--r--) to 0755
//! (rwxr-xr-x)`. Run recursively over a large tree this is hundreds of
//! near-identical lines. The signal is *how many* files were touched plus
//! a sample; the noise is the long homogeneous middle.
//!
//! chmod's message text is fully localised (`le mode de … a été modifié`,
//! `Modus von … geändert`, …), so we must NOT key off any English word.
//! The locale-INVARIANT shape is the octal mode token (`0755`) paired with
//! a 9-char symbolic permission block in parentheses (`(rwxr-xr-x)`),
//! which chmod emits identically in every locale. We fold runs of those
//! lines into a count, keeping head and tail samples.


const FOLD_THRESHOLD: usize = 24;
const KEEP_HEAD: usize = 8;
const KEEP_TAIL: usize = 4;

/// A 4-char octal mode token like `0644` / `0755`.
fn has_octal_mode(line: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| tok.len() == 4 && tok.starts_with('0') && tok.bytes().all(|b| (b'0'..=b'7').contains(&b)))
}

/// A parenthesised 9-char symbolic permission block like `(rwxr-xr-x)`.
fn has_perm_paren(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(open) = line[i..].find('(') {
        let start = i + open + 1;
        let block: String = line[start..].chars().take(9).collect();
        if block.chars().count() == 9
            && block.chars().all(|c| matches!(c, 'r' | 'w' | 'x' | 's' | 'S' | 't' | 'T' | '-'))
            && line[start + block.len()..].starts_with(')')
        {
            return true;
        }
        i = start;
        if i >= bytes.len() {
            break;
        }
    }
    false
}

/// A verbose chmod report line: carries both an octal mode and a symbolic
/// permission block — the locale-invariant fingerprint.
fn is_mode_line(line: &str) -> bool {
    has_octal_mode(line) && has_perm_paren(line)
}

pub fn detect(text: &str) -> bool {
    text.lines().filter(|l| is_mode_line(l)).count() >= 3
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.iter().filter(|l| is_mode_line(l)).count();
    if total <= FOLD_THRESHOLD {
        return Some(crate::archetypes::passthrough(text));
    }

    Some(crate::archetypes::fold_runs(
        text,
        &lines,
        is_mode_line,
        FOLD_THRESHOLD,
        KEEP_HEAD,
        KEEP_TAIL,
        "more file(s) changed",
    ))
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use super::*;

    #[test]
    fn detector_rejects_empty_and_plain() {
        assert!(!detect(""));
        assert!(!detect("some\nunrelated\noutput\n"));
    }

    #[test]
    fn matches_english_and_localised_lines() {
        let en = "mode of 'a' changed from 0644 (rw-r--r--) to 0755 (rwxr-xr-x)";
        let fr = "le mode de 'a' a été modifié de 0644 (rw-r--r--) en 0755 (rwxr-xr-x)";
        assert!(is_mode_line(en), "english line recognised");
        assert!(is_mode_line(fr), "french line recognised by structure");
    }

    #[test]
    fn folds_long_recursive_run() {
        let mut t = String::new();
        for i in 0..60 {
            let _ = writeln!(
                t,
                "le mode de 'src/file_{i:02}.txt' a été modifié de 0644 (rw-r--r--) en 0755 (rwxr-xr-x)"
            );
        }
        let out = try_compact(&t).unwrap();
        assert!(out.contains("file_00.txt"), "head bound kept");
        assert!(out.contains("file_59.txt"), "tail bound kept");
        assert!(out.contains("more file(s) changed"));
        assert!(!out.contains("file_30.txt"), "middle folded");
        assert!(out.len() < t.len(), "must shrink");
    }

    #[test]
    fn short_change_passes_through() {
        // Detected (>=3 mode lines) but under the fold threshold.
        let t = "mode of 'a' changed from 0644 (rw-r--r--) to 0755 (rwxr-xr-x)\n\
                 mode of 'b' changed from 0644 (rw-r--r--) to 0755 (rwxr-xr-x)\n\
                 mode of 'c' changed from 0644 (rw-r--r--) to 0755 (rwxr-xr-x)\n";
        let out = try_compact(t).unwrap();
        assert!(out.contains("0755"));
        assert!(!out.contains("changed) ..."));
    }
}
