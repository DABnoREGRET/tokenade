//! `diff` — diff.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
const HEAD: usize = 50;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| l.starts_with('+') || l.starts_with('-') || l.starts_with("@@") || l.starts_with('<') || l.starts_with('>'))
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    // The archetype decides WHETHER this output is worth folding at all — a
    // short diff still passes through untouched. Only once it has said yes
    // does the shape matter: a unified diff goes to the parser that keeps
    // every change, because the archetype keeps the first N lines and the
    // last 5, and a diff's changed lines are all it carries. See
    // `crate::diff::compact_if_unified`.
    let folded = crate::archetypes::form_aware(text, HEAD, 5, "diff", "line(s)")?;
    // The NORMAL format (`diff a b`, no `-u`) is the same rule in another
    // spelling: change commands, `<`/`>` lines and `---`, nothing to cut. The
    // dispatcher serves it whole BEFORE reaching here (`filter_command`'s
    // `("diff", _) if is_normal_diff(…)` arm): a `Some` equal to the input is
    // refused by the named-dispatch inflation guard and handed to `auto`.
    if served_whole(text) {
        return None;
    }
    Some(crate::diff::compact_if_unified(text).unwrap_or(folded))
}

/// `diff -y`: the WHOLE file, side by side, a marker column flagging the
/// changed rows (`|` changed, `<` left only, `>` right only). Every changed row
/// is kept with one row of context on each side; runs of unchanged rows fold
/// to the standard elision marker (recoverable).
///
/// The marker column is FOUND, not assumed: the most frequent column, tabs
/// expanded, where `|`/`<`/`>` stands after a space. A row is changed iff it
/// carries the marker at THAT column, so code with ` | ` elsewhere on an
/// unchanged row cannot pass for a change. Measured 2026-10-02: a 4 281-line
/// `diff -y` came back as 68 lines with 6 of its 186 changed rows.
pub fn fold_side_by_side(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let expanded: Vec<Vec<char>> = lines
        .iter()
        .map(|l| {
            let mut out = Vec::with_capacity(l.len());
            for c in l.chars() {
                if c == '\t' {
                    out.push(' ');
                    while out.len() % 8 != 0 {
                        out.push(' ');
                    }
                } else {
                    out.push(c);
                }
            }
            out
        })
        .collect();
    let is_marker = |e: &[char], i: usize| {
        i > 0
            && matches!(e.get(i), Some('|' | '<' | '>'))
            && e[i - 1] == ' '
            && e.get(i + 1).is_none_or(|c| *c == ' ')
    };
    let mut votes: std::collections::HashMap<usize, usize> = Default::default();
    for e in &expanded {
        for i in 1..e.len() {
            if is_marker(e, i) {
                *votes.entry(i).or_default() += 1;
            }
        }
    }
    let (&col, _) = votes.iter().max_by_key(|(c, n)| (**n, std::cmp::Reverse(**c)))?;
    let mut keep = std::collections::BTreeSet::new();
    for (i, e) in expanded.iter().enumerate() {
        if is_marker(e, col) {
            keep.extend(i.saturating_sub(1)..=(i + 1).min(lines.len() - 1));
        }
    }
    if keep.is_empty() || keep.len() * 10 > lines.len() * 8 {
        return None;
    }
    let indices: Vec<usize> = keep.into_iter().collect();
    Some(crate::elision::render_kept(&lines, &indices))
}

/// Did the caller ask for side-by-side output (`-y`, `--side-by-side`, or a
/// short-flag cluster carrying `y` such as `-yw`)?
pub fn asks_side_by_side(argv: &[String]) -> bool {
    argv.iter().skip(1).any(|a| {
        a == "--side-by-side" || (a.starts_with('-') && !a.starts_with("--") && a[1..].contains('y'))
    })
}

/// The diff formats whose lines are all the reader needs and that nothing
/// here knows how to shorten without losing a change: NORMAL (`diff a b`) and
/// CONTEXT (`diff -c`). `filter_command` serves these whole.
pub fn served_whole(text: &str) -> bool {
    is_normal_diff(text) || is_context_diff(text)
}

/// A line `diff` itself writes AROUND the hunks when it compares trees
/// (`-r`): the per-file command echo (`diff -r a/x b/x`), `Only in a: f`,
/// `Common subdirectories: a/d and b/d`, `Binary files a and b differ`.
///
/// Both detectors below read hunks and refused ANY other line, so the
/// recursive form of the very formats they serve whole was not recognised:
/// measured 2026-10-02, `diff -r` of `tk-formats/src` across 60 commits (350
/// lines, six files) came back as 58 lines naming two of the six files.
///
/// Recognised by SHAPE, not by its words: those messages are translated
/// (`Seulement dans …` here, `Nur in …` in German), and an English list let a
/// French `diff -r` of 36 files — 5 116 lines — come back as 68 lines naming
/// one. A framing line is one that can be neither a change command (those
/// start with a digit) nor a line of a hunk (`<` `>` `-` `+` `!` `*` `\` or
/// a space). [`hunks_and_framing`] bounds how many there may be, so prose
/// with one stray `3a4` is still not a diff.
pub(crate) fn is_diff_framing(l: &str) -> bool {
    !l.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '<' | '>' | '-' | '+' | '!' | '*' | '\\' | ' '))
}

/// The non-empty lines that are not framing — `None` when framing outweighs
/// them, i.e. the text is not a diff with some framing but something else.
fn hunks_and_framing(text: &str) -> Option<Vec<&str>> {
    let (framing, body): (Vec<&str>, Vec<&str>) =
        text.lines().filter(|l| !l.trim().is_empty()).partition(|l| is_diff_framing(l));
    (framing.len() <= body.len()).then_some(body)
}

/// `diff -c`: `*** a` / `--- b` headers, `***************` between hunks,
/// `*** 1,5 ****` / `--- 1,6 ----` ranges, then lines marked `  `, `- `, `+ `,
/// `! ` or `\ No newline…`. Measured 2026-10-02: a 209-line `diff -c` came
/// back as 58 lines, 61 of its 186 changed lines left.
pub fn is_context_diff(text: &str) -> bool {
    let Some(body) = hunks_and_framing(text) else {
        return false;
    };
    let mut lines = body.into_iter();
    let (Some(a), Some(b)) = (lines.next(), lines.next()) else {
        return false;
    };
    if !(a.starts_with("*** ") && b.starts_with("--- ")) {
        return false;
    }
    let mut hunks = 0usize;
    for l in lines {
        if l.starts_with("***************") {
            hunks += 1;
        } else if !(l.starts_with("*** ")
            || l.starts_with("--- ")
            || l.starts_with("  ")
            || l.starts_with("- ")
            || l.starts_with("+ ")
            || l.starts_with("! ")
            || l.starts_with("\\ ")
            || matches!(l, "-" | "+" | "!"))
        {
            return false;
        }
    }
    hunks > 0
}

/// Every non-empty line is a change command (`130a131,164`, `5,7c5`,
/// `12d11`), a changed line (`< `, `> `), the `---` between the two sides of a
/// change, or `\ No newline at end of file`.
pub fn is_normal_diff(text: &str) -> bool {
    let command = |l: &str| {
        let Some(i) = l.find(['a', 'c', 'd']) else {
            return false;
        };
        let range = |r: &str| {
            !r.is_empty() && r.split(',').count() <= 2 && r.split(',').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        };
        range(&l[..i]) && range(&l[i + 1..])
    };
    let mut commands = 0usize;
    let Some(body) = hunks_and_framing(text) else {
        return false;
    };
    for l in body {
        if command(l) {
            commands += 1;
        } else if !(l.starts_with('<') || l.starts_with('>') || l == "---" || l.starts_with("\\ ")) {
            return false;
        }
    }
    commands > 0
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    /// `diff -r` writes its own lines between the hunks of each file. They
    /// are framing, not content, and the recursive form is served whole like
    /// the single-file one — every file named, every change kept.
    #[test]
    fn a_recursive_diff_is_served_whole_with_its_framing() {
        let normal = "diff -r old/src/env.rs src/env.rs\n11c11\n< //!  `first 60 chars … [+N total chars]`\n---\n> //!  `first 60 chars…[+N chars]`\n\
                      186a187,188\n> /// `head…[+N chars]`, N being what was CUT.\n> fn cut_marker() {}\n\
                      Only in src/json: walk_tests.rs\n\
                      Common subdirectories: old/src/logs and src/logs\n\
                      Binary files old/src/fixture.bin and src/fixture.bin differ\n\
                      diff -r old/src/json/fold.rs src/json/fold.rs\n21a22\n> pub const SMALL_ARRAY_BYTES: usize = 6_000;\n\
                      340,342c348,353\n<     let elided = matching.len().saturating_sub(3);\n---\n>     let elided = matching.len().saturating_sub(keep);\n";
        assert!(served_whole(normal));
        let context = "diff -rc old/a.rs a.rs\n*** old/a.rs\t2026-10-01\n--- a.rs\t2026-10-02\n***************\n*** 1,2 ****\n! let x = 1;\n  fn f() {}\n--- 1,2 ----\n! let x = 2;\n  fn f() {}\n\
                       Only in old: gone.rs\n";
        assert!(served_whole(context));
        let argv: Vec<String> = ["diff", "-r", "old/src", "src"].iter().map(|s| s.to_string()).collect();
        assert_eq!(crate::filter::filter_command(&argv, normal).output, normal);
        // Framing alone is not a diff this file serves.
        assert!(!served_whole("Only in a: x\nOnly in b: y\n"));

        // The messages are translated; the shape is not.
        let fr = normal
            .replace("Only in src/json: walk_tests.rs", "Seulement dans src/json: walk_tests.rs")
            .replace("Common subdirectories: old/src/logs and src/logs", "Les sous-répertoires old/src/logs et src/logs sont identiques")
            .replace("Binary files old/src/fixture.bin and src/fixture.bin differ", "Les fichiers binaires old/src/fixture.bin et src/fixture.bin sont différents");
        assert!(served_whole(&fr), "{fr}");
        assert!(served_whole(&normal.replace("Only in src/json:", "Nur in src/json:")));

        // Prose with one stray command-shaped line is still prose: framing
        // may not outweigh the hunks.
        let prose = "Notes from the review.\nThe table moved to section 3.\n3a4\n> added\nEverything else is unchanged.\nSee the appendix.\n";
        assert!(!served_whole(prose));
    }

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("+added line")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("+added line", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    /// A NORMAL diff has only changes: nothing to cut (12 changed
    /// lines out of 64 lost before, 2026-10-02).
    #[test]
    fn a_normal_diff_keeps_every_change() {
        let mut s = String::from("130a131,200\n");
        for i in 0..70 {
            s.push_str(&format!("> line {i} of the added block\n"));
        }
        s.push_str("210,211c280,281\n< old a\n< old b\n---\n> new a\n> new b\n");
        assert!(is_normal_diff(&s));
        assert!(try_compact(&s).is_none(), "never folded here");
        assert!(!is_normal_diff("+added line\n+added line\n"));
    }

    #[test]
    fn a_context_diff_is_recognised() {
        let s = "*** a.rs\t2026-10-02\n--- b.rs\t2026-10-02\n***************\n*** 1,3 ****\n  keep\n! old\n  keep\n--- 1,3 ----\n  keep\n! new\n  keep\n";
        assert!(is_context_diff(s) && served_whole(s));
        assert!(!is_context_diff("*** not a diff\nplain text\n"));
    }

    /// A `diff -y` folds the unchanged and keeps every marked row, even
    /// when an unchanged row carries ` | ` elsewhere (a Rust closure).
    #[test]
    fn side_by_side_keeps_every_marked_row() {
        let row = |l: &str, m: char, r: &str| format!("{l:<61} {m} {r}\n");
        let mut s = String::new();
        for i in 0..200 {
            let code = format!("let f = |x| x + {i};");
            s.push_str(&row(&code, ' ', &code));
        }
        s.push_str(&row("old_call();", '|', "new_call();"));
        s.push_str(&row("only_left();", '<', ""));
        for i in 0..200 {
            s.push_str(&row(&format!("tail_{i}();"), ' ', &format!("tail_{i}();")));
        }
        let out = fold_side_by_side(&s).expect("folded");
        assert!(out.contains("old_call();") && out.contains("new_call();"));
        assert!(out.contains("only_left();"));
        assert!(out.contains("elided by tokenade"), "{out}");
        assert!(out.lines().count() < 20, "{}", out.lines().count());
        assert!(asks_side_by_side(&["diff".into(), "-yw".into()]));
        assert!(!asks_side_by_side(&["diff".into(), "-u".into()]));
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = std::iter::repeat_n("+added line", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:diff"));
    }
}
