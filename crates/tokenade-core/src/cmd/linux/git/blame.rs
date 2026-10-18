//! Extracted from `filter.rs` in the L92 refactor.
//!
//! Each function below was hand-tuned in filter.rs; the body is//! preserved verbatim — only the home changed.
use crate::compress::generic;
use crate::compress::truncate_middle;
use crate::ansi::strip_ansi;

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    let mut compact = Vec::new();
    let mut authors: Vec<String> = Vec::new();
    let mut dates: Vec<String> = Vec::new();
    // Per COMMIT, in first-seen order: author and date belong to the commit,
    // so one legend line per commit gives every kept line its attribution back.
    let mut commits: Vec<(String, String, String)> = Vec::new();
    let mut line_numbers: Vec<usize> = Vec::new();
    for line in s.lines() {
        // Format: <hash> (<author> <date> <linenum>) <code>
        // The closing `)` is the divider. Find it, strip everything
        // between `(` and `)`, keep hash + code.
        if let Some(open_paren) = line.find(" (") {
            if let Some(close_paren) = line[open_paren..].find(") ") {
                let hash = &line[..open_paren];
                let code = &line[open_paren + close_paren + 2..];
                // The `(author date lineno)` block is what we drop, and it is
                // the whole point of `blame`. Keep it per line and the saving
                // falls from 36% to 8% (MEASURED: 12,437 bytes become 17,796
                // against 19,427 raw), so dropping it stays the right trade —
                // but it has to be SAID, and the two banners this output
                // already carries speak only of folded LINES, never of fields
                // removed from the lines that were kept.
                let meta = &line[open_paren + 2..open_paren + close_paren];
                // The author is everything BEFORE the date, not the first
                // word: `Alice Author 2024-01-15 …` is one person, and taking
                // the first token alone would report two contributors as one
                // and name neither correctly.
                let short_hash: String = hash.chars().take(8).collect();
                if let Some(n) = meta.split_whitespace().last().and_then(|t| t.parse().ok()) {
                    line_numbers.push(n);
                }
                if let Some((name, date)) = split_author_date(meta) {
                    if !commits.iter().any(|(h, _, _)| *h == short_hash) {
                        commits.push((short_hash.clone(), name.clone(), date.clone()));
                    }
                    authors.push(name);
                    dates.push(date);
                } else if let Some(first) = meta.split_whitespace().next() {
                    authors.push(first.to_string());
                }
                compact.push(format!("{short_hash} {code}"));
                continue;
            }
        }
        compact.push(line.to_string());
    }
    if compact.is_empty() {
        return generic(output);
    }
    let body = truncate_middle(&compact.join("\n"), 80, 20);
    let legend = commit_legend(&commits, &line_numbers);
    let note = attribution_note(&authors, &dates).map(|n| match &legend {
        Some(l) => n.replace(
            " · per-line attribution: re-run the command through `tokenade raw`",
            &format!(" · per line: the commit's hash, dated below{l}"),
        ),
        None => n,
    });
    match note {
        Some(note) => format!("{note}\n{body}"),
        None => body,
    }
}

/// Commits a blame may name one legend line each before the summary is used.
const LEGEND_MAX_COMMITS: usize = 20;

/// **Author and date are the COMMIT's, said once per commit.** The summary
/// (`1 author(s) · 4 distinct date(s), 2026-06-01..2026-10-02`) answered
/// "who" and "since when" but not "which line changed when" — the per-line
/// question `blame` is run for, sent to `tokenade raw`. Measured 2026-10-03,
/// `git blame -L 1,80 dns.rs`: 4 commits, so 4 legend lines restore every
/// line's attribution. The line numbers, dropped with the block, come back as
/// a range when they are contiguous (`-L 400,480` gave no way to know).
fn commit_legend(commits: &[(String, String, String)], lines: &[usize]) -> Option<String> {
    if commits.is_empty() || commits.len() > LEGEND_MAX_COMMITS {
        return None;
    }
    let one_author = commits.iter().all(|(_, a, _)| *a == commits[0].1);
    let contiguous = lines.windows(2).all(|w| w[1] == w[0] + 1);
    let mut out = String::new();
    if let (true, Some(first), Some(last)) = (contiguous, lines.first(), lines.last()) {
        out.push_str(&format!(" · lines {first}–{last}"));
    }
    for (h, a, d) in commits {
        if one_author {
            out.push_str(&format!("\n  {h} {d}"));
        } else {
            out.push_str(&format!("\n  {h} {d} {a}"));
        }
    }
    Some(out)
}

/// `Alice Author 2024-01-15 10:30:00 +0000  42` → `("Alice Author", "2024-01-15")`.
fn split_author_date(meta: &str) -> Option<(String, String)> {
    let mut name: Vec<&str> = Vec::new();
    for tok in meta.split_whitespace() {
        if is_iso_date(tok) {
            if name.is_empty() {
                return None;
            }
            return Some((name.join(" "), tok.to_string()));
        }
        name.push(tok);
    }
    None
}

fn is_iso_date(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Say what was taken off every kept line, and answer the question `blame` was
/// asked while doing it.
///
/// A summary is not a consolation prize here: authorship repeats heavily, so
/// naming the authors with their line counts and the date span usually IS the
/// answer, at 1.5% of the output (187 bytes on 12,437 measured). Per-line
/// attribution is one re-run away and the note says so.
fn attribution_note(authors: &[String], dates: &[String]) -> Option<String> {
    if authors.is_empty() && dates.is_empty() {
        return None;
    }
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for a in authors {
        *counts.entry(a.as_str()).or_default() += 1;
    }
    let mut top: Vec<(&str, usize)> = counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let named = top.len();
    let shown: Vec<String> = top
        .iter()
        .take(3)
        .map(|(a, n)| format!("{a} ({n})"))
        .collect();
    let more = if named > 3 {
        format!(", +{} more", named - 3)
    } else {
        String::new()
    };
    let span = if dates.is_empty() {
        String::new()
    } else {
        let mut d: Vec<&str> = dates.iter().map(String::as_str).collect();
        d.sort_unstable();
        let distinct = {
            let mut u = d.clone();
            u.dedup();
            u.len()
        };
        format!(
            " · {distinct} distinct date(s), {}..{}",
            d[0],
            d[d.len() - 1]
        )
    };
    Some(format!(
        "[tokenade:git-blame] author and date removed from every line · \
         {named} author(s): {}{more}{span} · per-line attribution: re-run the \
         command through `tokenade raw`",
        shown.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    // #504 — lock-in for the git_blame compactor (zero coverage prior).
    // Hand-picked fixtures cover the standard parse, the malformed-line
    // passthrough, ANSI-coloured input (`git blame --color-lines`), the
    // empty/no-paren fallback to `generic`, and the head/tail truncation
    // on huge blames.

    #[test]
    fn strips_author_date_block_keeps_short_hash_and_code() {
        let input = "\
3f8a4b2c1d (Alice Author 2024-01-15 10:30:00 +0000  42) fn foo() {
3f8a4b2c1d (Alice Author 2024-01-15 10:30:00 +0000  43)     bar()
9abcdef012 (Bob   Maint  2024-02-20 09:15:11 +0100  44) }
";
        let out = compact(input);
        // Short hash kept (8 chars).
        assert!(out.contains("3f8a4b2c "), "short hash missing: {out}");
        assert!(out.contains("9abcdef0 "), "second short hash missing: {out}");
        // Code preserved.
        assert!(out.contains("fn foo() {"), "code line dropped");
        assert!(out.contains("    bar()"), "indented code dropped");
        assert!(out.contains('}'), "closing brace dropped");
        // The per-LINE metadata block is gone — that is what the assertion
        // below has always meant. It used to forbid the strings ANYWHERE,
        // which the declaring note now breaks on purpose: an output that
        // removes attribution has to say whose it was.
        // The banner and its per-commit legend (`  <hash> <date> <author>`)
        // are where attribution now lives; the CODE lines must carry none.
        for l in out.lines().filter(|l| !l.starts_with("[tokenade") && !l.starts_with("  ")) {
            assert!(!l.contains("+0000"), "tz leaked on a code line: {l}");
            assert!(!l.contains("10:30:00"), "clock leaked on a code line: {l}");
            assert!(
                !l.contains("Alice Author"),
                "author leaked on a code line: {l}"
            );
        }

        // …and the removal is DECLARED, with the answer `blame` was asked for.
        assert!(
            out.contains("[tokenade:git-blame]"),
            "the strip must announce itself: {out}"
        );
        assert!(
            out.contains("Alice Author (2)") && out.contains("Bob Maint (1)"),
            "the note must name who wrote how many lines: {out}"
        );
        assert!(
            out.contains("2024-01-15..2024-02-20"),
            "the note must give the date span: {out}"
        );
    }

    #[test]
    fn malformed_line_with_no_parens_passes_through() {
        // A `git blame --line-porcelain` header / mid-output noise line
        // without `(` / `)` should land verbatim (we have no parse to
        // apply but we must NOT drop it — losing data is worse than
        // losing compression on one line).
        let input = "author Alice\nauthor-mail <alice@example.com>\n3f8a4b2c (Alice 2024 1) code\n";
        let out = compact(input);
        assert!(
            out.contains("author Alice"),
            "non-blame header line dropped: {out}"
        );
        assert!(
            out.contains("author-mail"),
            "second non-blame line dropped: {out}"
        );
        assert!(out.contains("3f8a4b2c"), "real blame line dropped");
        assert!(out.contains("code"), "blame code lost");
    }

    #[test]
    fn ansi_colour_codes_are_stripped_before_parse() {
        // `git blame --color-lines` wraps the hash in ANSI escapes —
        // without strip_ansi, the leading ESC chars break the
        // `line.find(" (")` index math and the hash leaks coloured.
        let input = "\x1b[31m3f8a4b2c\x1b[0m (Alice 2024 1) fn x() {}\n";
        let out = compact(input);
        assert!(!out.contains('\x1b'), "ANSI escapes leaked: {out:?}");
        assert!(out.contains("fn x() {}"), "code dropped: {out}");
    }

    #[test]
    fn empty_input_falls_through_to_generic() {
        // No lines → compact vec is empty → falls back to generic
        // (which trim_end's the empty input to empty).
        assert_eq!(compact(""), "");
    }

    #[test]
    fn very_long_blame_is_truncated_middle() {
        // truncate_middle keeps 80 head + 20 tail lines. Generate 200
        // blame lines and verify the middle was elided.
        let mut input = String::new();
        for i in 0..200 {
            let _ = writeln!(
                input,
                "3f8a4b2c (Alice 2024 {i}) line_{i}"
            );
        }
        let out = compact(&input);
        assert!(out.contains("line_0"), "head dropped");
        assert!(out.contains("line_199"), "tail dropped");
        assert!(
            !out.contains("line_100"),
            "middle should have been elided: {}",
            out.lines().count()
        );
        assert!(out.contains("elided by tokenade"), "elision marker missing");
    }

    /// Real `git blame -L 1,80`: one author, four commits. Each commit is
    /// named once with its date, the line range is said, the code lines keep
    /// only their hash.
    #[test]
    fn each_commit_is_named_once_with_its_date_and_the_range_is_said() {
        let input = "\
^584eb0f6 (pi-infected 2026-06-01 21:31:35 +0200 41) //! DNS lookup
ec8704af (pi-infected 2026-10-03 04:30:58 +0200 42) let x = 1;
^584eb0f6 (pi-infected 2026-06-01 21:31:35 +0200 43) }
";
        let out = compact(input);
        assert_eq!(
            out,
            "[tokenade:git-blame] author and date removed from every line · 1 author(s): pi-infected (3) · 2 distinct date(s), 2026-06-01..2026-10-03 · per line: the commit's hash, dated below · lines 41–43\n  \
             ^584eb0f 2026-06-01\n  ec8704af 2026-10-03\n^584eb0f //! DNS lookup\nec8704af let x = 1;\n^584eb0f }"
        );
    }
}
