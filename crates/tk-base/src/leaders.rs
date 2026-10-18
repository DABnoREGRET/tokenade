//! **Dot leaders carry nothing.**
//!
//! A two-column view aligned with dots — `api/users ........ users.index`
//! (Laravel's `artisan`), `Préface ................ 7` (a table of contents in
//! a PDF) — spends most of its bytes reaching the right margin. Measured
//! 2026-10-02: half of a 50-route `route:list` (3 978 of 7 772 bytes); 9 % of
//! the text of a 40-page specification (7 276 of 83 399 bytes, its contents
//! page). One rule, two callers: the `artisan` compactor and document
//! extraction.

/// Every dot leader on `line` replaced by ` … `. A leader is a run of three
/// dots or more with a space before it and a space (or the end of the line)
/// after it, or a run of eight dots or more anywhere — `Préface.......7` as
/// some extractors glue it. A shorter run glued to a word (`wait...`,
/// `v1...v2`) is text and stays.
pub fn collapse_dot_leaders(line: &str) -> String {
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        if b[i] != b'.' {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i] == b'.' {
            i += 1;
        }
        let n = i - start;
        let spaced = start > 0 && b[start - 1] == b' ' && (i == b.len() || b[i] == b' ');
        if (n >= 3 && spaced) || n >= 8 {
            out.push_str(line[copied..start].trim_end());
            out.push_str(" … ");
            while i < b.len() && b[i] == b' ' {
                i += 1;
            }
            copied = i;
        }
    }
    if copied == 0 && out.is_empty() {
        return line.to_string();
    }
    out.push_str(&line[copied..]);
    out.trim_end().to_string()
}

/// [`collapse_dot_leaders`] over every line; `None` when nothing changed.
pub fn collapse_dot_leaders_in(text: &str) -> Option<String> {
    if !text.contains("...") {
        return None;
    }
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let c = collapse_dot_leaders(line);
        changed |= c != line;
        out.push_str(if c != line { &c } else { line });
    }
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaders_go_text_stays() {
        assert_eq!(
            collapse_dot_leaders("  GET|HEAD  api/users ........ users.index"),
            "  GET|HEAD  api/users … users.index"
        );
        assert_eq!(
            collapse_dot_leaders("Préface .................... 7"),
            "Préface … 7"
        );
        assert_eq!(
            collapse_dot_leaders("Préface.................7"),
            "Préface … 7"
        );
        assert_eq!(
            collapse_dot_leaders("  wait... done ...."),
            "  wait... done …"
        );
        assert_eq!(
            collapse_dot_leaders("a ...b ...."),
            "a ...b …",
            "dots glued to a word are text"
        );
        assert_eq!(collapse_dot_leaders("v1...v2"), "v1...v2");
        assert_eq!(collapse_dot_leaders_in("x\ny"), None);
        assert_eq!(
            collapse_dot_leaders("keeps its trailing space "),
            "keeps its trailing space "
        );
    }
}
