//! A uniform line-number gutter, and how to see past it.
//!
//! `cat -n`, `nl`, and every agent Read tool that numbers its output put a
//! sequential integer in front of each line. That gutter defeats the WHOLE
//! pipeline, not one stage of it: the format detector no longer recognises the
//! document, the generic deduper sees every line as unique because the number
//! differs, and the disclose summary never fires.
//!
//! Measured 2026-08-26 on a 26 KB `.mdx` article:
//!
//! | input                    | delivered |
//! |--------------------------|-----------|
//! | `cat FILE`               | 3 294 B   |
//! | `cat -n FILE`            | 14 743 B  |
//! | `read` of the numbered file | 27 653 B of a 27 652 B input |
//!
//! The last row is the shape of the defect: not a weaker fold, NO fold. It is
//! also the biggest single bucket in this machine's `generic` ledger — 261 000
//! tokens delivered across 70 `cat -n` calls.
//!
//! Third time a line-numbered payload has broken a reader here (a numbered
//! source once read as an anti-bot wall, a numbered Rust file once labelled
//! `.csv`), so the scan lives in one place rather than in each of them.

/// What a gutter covered, so the caller can say so rather than silently drop it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gutter {
    pub first: u64,
    pub last: u64,
    pub lines: usize,
}

/// Fewest numbered lines before a leading integer is a GUTTER rather than a
/// coincidence. A handful of lines that happen to start `1`, `2`, `3` is an
/// ordered list, not `cat -n` output.
const MIN_LINES: usize = 8;

/// Split a line-number gutter off `text`, returning what it covered and the
/// text without it. `None` when there is no gutter — which is the answer for
/// almost every input, so the scan gives up on the first line that disagrees.
///
/// Deliberately strict, because guessing wrong DELETES the first field of every
/// line:
///
/// * the separator is a TAB (`cat -n`, `nl`) or `→` (agent Read tools). `:` is
///   excluded on purpose — that is `grep -n`, which has its own compactor, and
///   it is also how YAML and countless logs spell an ordinary field.
/// * the numbers rise by EXACTLY one, with no gaps. This is what separates a
///   gutter from a document that happens to begin its lines with integers: a
///   numbered list restarts, a table's ids skip, a log's counters jump.
/// * every non-empty line carries one. One line without is enough to decline.
pub fn split(text: &str) -> Option<(Gutter, String)> {
    let mut expected: Option<u64> = None;
    let mut first = 0u64;
    let mut n = 0usize;
    let mut body = String::with_capacity(text.len());
    for line in text.lines() {
        if line.trim().is_empty() {
            // A blank line inside numbered output still carries its number, so
            // a truly empty one means this is not `cat -n`. `cat -n` numbers
            // blank lines too; `nl` by default does not, and prints them bare.
            body.push('\n');
            continue;
        }
        let (num, rest) = strip_one(line)?;
        match expected {
            None => {
                first = num;
                expected = Some(num + 1);
            }
            Some(want) => {
                if num != want {
                    return None;
                }
                expected = Some(num + 1);
            }
        }
        n += 1;
        body.push_str(rest);
        body.push('\n');
    }
    if n < MIN_LINES {
        return None;
    }
    Some((
        Gutter {
            first,
            last: expected? - 1,
            lines: n,
        },
        body,
    ))
}

/// `text` with the gutter taken off every line that carries one, or `None`
/// unless MOST non-empty lines do.
///
/// Laxer than [`split`] on purpose, because it answers a different question:
/// [`split`] DELETES the gutter, so it insists on a contiguous run of eight and
/// on every line; this one only lets a caller judge what the gutter numbers.
/// `nl` without `-ba` prints blank lines bare, and a folded view interleaves
/// lines of its own (`… [52 lines elided by tokenade …] …`) — measured
/// 2026-10-02, one such marker was enough to send numbered code back to being
/// judged as a table and flattened.
pub fn without_gutter(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let (mut numbered, mut other) = (0usize, 0usize);
    for line in text.lines() {
        match strip_one(line) {
            Some((_, rest)) => {
                numbered += 1;
                out.push_str(rest);
            }
            None => {
                if !line.trim().is_empty() {
                    other += 1;
                }
                out.push_str(line);
            }
        }
        out.push('\n');
    }
    (numbered > other).then_some(out)
}

/// The number and the rest of one line, or `None` if this line has no gutter.
pub fn strip_one(line: &str) -> Option<(u64, &str)> {
    let after_pad = line.trim_start_matches([' ', '\t']);
    let digits: String = after_pad
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() || digits.len() > 9 {
        return None;
    }
    let rest = &after_pad[digits.len()..];
    // The separator, and it must be there: `12abc` is a word, not a gutter.
    let rest = rest
        .strip_prefix('\t')
        .or_else(|| rest.strip_prefix('\u{2192}'))?;
    Some((digits.parse().ok()?, rest))
}

/// `text` with its gutter written as [`renumber`] writes it — the number, not
/// the padding (`     1\t` → `1\t`). `None` when there is no gutter or nothing
/// to gain.
///
/// A numbered read that FITS was delivered untouched, padding included, while
/// one a fold had rewritten came back unpadded: the same question answered in
/// two formats, and the padding is pure cost. Measured 2026-10-03 on
/// `nl -ba remote_argv.rs | sed -n '1,260p'` — the form Codex reads code in,
/// `nl` 7 900 events on the fleet at 4 % coverage: 3 512 → 3 215 tokens
/// (−8.5 %), every number and every byte of code kept.
pub fn unpadded(text: &str) -> Option<String> {
    let (_, body) = split(text)?;
    let mut out = renumber(&body, text);
    if !text.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out.len() < text.len()).then_some(out)
}

/// Put the gutter back on every line of `compacted` that the compactor
/// delivered VERBATIM, with the number that line had in `numbered`.
///
/// The gutter comes off so the pipeline can fold at all, and it used to stay
/// off: `cat -n guard.ts | sed -n '1,120p'` came back without a single
/// number, after a fold that rewrote and removed lines — so the numbers the
/// agent asked for (to cite, to `sed -n` a range, to Edit) could not even be
/// counted back. Written unpadded (`51\t…`, which [`strip_one`] reads back):
/// the number is what was asked for, the padding is not. Matching walks forward only, so a repeated line (`}`) takes
/// the next occurrence after the previous match; a line the compactor wrote
/// itself (a marker, a folded template) matches nothing and stays bare.
pub fn renumber(compacted: &str, numbered: &str) -> String {
    let source: Vec<(u64, &str)> = numbered.lines().filter_map(strip_one).collect();
    // Positions of each distinct line, ascending: the next occurrence at or
    // after the cursor is one binary search, not a scan of the rest — the scan
    // took 99.7 s on 60 000 lines a compactor had rewritten (nothing matched).
    let mut positions: std::collections::HashMap<&str, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, (_, l)) in source.iter().enumerate() {
        positions.entry(l.trim_end()).or_default().push(i);
    }
    let mut at = 0usize;
    let mut out = String::with_capacity(compacted.len() + compacted.lines().count() * 6);
    for line in compacted.lines() {
        let want = line.trim_end();
        let hit = (!want.is_empty())
            .then(|| positions.get(want))
            .flatten()
            .and_then(|ps| ps.get(ps.partition_point(|&p| p < at)).copied());
        match hit {
            Some(i) => {
                let (n, _) = source[i];
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{n}\t{line}\n"));
                at = i + 1;
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !compacted.ends_with('\n') {
        out.pop();
    }
    out
}

/// One line naming the gutter that was removed, in the same shape the archive
/// compactor uses for the fields it drops.
pub fn note(g: &Gutter) -> String {
    format!(
        "[tokenade:line-numbers] gutter {}-{} ({} lines) set aside to compact, \
         restored on every line delivered verbatim; `--raw` on the recovery \
         pointer returns the numbered bytes\n",
        g.first, g.last, g.lines
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The padding goes, every number and every byte of the line stays —
    /// including a tab-indented body, which a whitespace trim would eat.
    #[test]
    fn unpadded_keeps_numbers_and_lines_and_drops_only_the_padding() {
        let text: String = (1..=12)
            .map(|n| format!("{n:>6}\t\tfn f{n}() {{}}\n"))
            .collect();
        let out = unpadded(&text).expect("a 12-line gutter");
        let want: String = (1..=12)
            .map(|n| format!("{n}\t\tfn f{n}() {{}}\n"))
            .collect();
        assert_eq!(out, want);
        assert_eq!(
            unpadded("just\nsome\ntext\n"),
            None,
            "no gutter, no rewrite"
        );
        let unpadded_already: String = (1..=12).map(|n| format!("{n}\tx\n")).collect();
        assert_eq!(unpadded(&unpadded_already), None, "nothing to gain");
    }

    /// Linear, not quadratic: a compactor that rewrote every line (nothing
    /// matches) must not make each line scan the rest of the input.
    #[test]
    fn renumber_is_not_quadratic_when_nothing_matches() {
        let n = 60_000;
        let numbered: String = (1..=n)
            .map(|i| format!("{i:>6}\tsource line {i} with text\n"))
            .collect();
        let compacted: String = (1..=n).map(|i| format!("rewritten {i}\n")).collect();
        let t = std::time::Instant::now();
        let out = renumber(&compacted, &numbered);
        assert_eq!(out.lines().count(), n);
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
    }

    /// Every line delivered verbatim gets its own number back; a line the
    /// compactor wrote stays bare; a repeated line takes the NEXT occurrence.
    #[test]
    fn renumber_restores_the_number_of_each_verbatim_line() {
        let numbered = "    40\texport function step5(ctx: Ctx): boolean {\n    41\t  return ctx.ok;\n    42\t}\n    43\t\n    44\texport function step6(ctx: Ctx): boolean {\n    45\t  return !ctx.ok;\n    46\t}\n";
        let compacted = "export function step5(ctx: Ctx): boolean {\n… [1 lines elided by tokenade] …\n}\nexport function step6(ctx: Ctx): boolean {\n}\n";
        let out = renumber(compacted, numbered);
        assert_eq!(
            out,
            "40\texport function step5(ctx: Ctx): boolean {\n… [1 lines elided by tokenade] …\n42\t}\n44\texport function step6(ctx: Ctx): boolean {\n46\t}\n"
        );
    }

    fn numbered(n: usize, start: u64) -> String {
        (0..n)
            .map(|i| format!("{:>6}\tline {i} of the document\n", start + i as u64))
            .collect()
    }

    #[test]
    fn cat_dash_n_output_is_recognised() {
        let t = numbered(40, 1);
        let (g, body) = split(&t).expect("a gutter");
        assert_eq!(
            g,
            Gutter {
                first: 1,
                last: 40,
                lines: 40
            }
        );
        assert!(body.starts_with("line 0 of the document\n"), "{body:.40}");
        assert!(!body.contains('\t'), "the gutter survived");
    }

    /// A slice (`sed -n '100,140p' | cat -n` is rare, but `nl -ba` from an
    /// offset and agent Read tools both start elsewhere) must work too.
    #[test]
    fn a_gutter_that_does_not_start_at_one_is_still_a_gutter() {
        let t = numbered(20, 340);
        let (g, _) = split(&t).unwrap();
        assert_eq!((g.first, g.last), (340, 359));
    }

    #[test]
    fn the_arrow_form_agent_read_tools_use_is_recognised() {
        let t: String = (1..=12)
            .map(|i| format!("{i:>4}\u{2192}code line {i}\n"))
            .collect();
        assert_eq!(split(&t).unwrap().0.lines, 12);
    }

    /// The three ways a leading integer is NOT a gutter. Guessing wrong here
    /// deletes the first field of every line.
    #[test]
    fn an_ordinary_leading_integer_is_not_a_gutter() {
        // A colon is `grep -n`, YAML, and a hundred log formats.
        let grep: String = (1..=20)
            .map(|i| format!("{i}:some matching line\n"))
            .collect();
        assert!(
            split(&grep).is_none(),
            "a `:` separator was taken for a gutter"
        );
        // Ids that skip.
        let ids: String = (0..20).map(|i| format!("{}\tuser row\n", i * 7)).collect();
        assert!(
            split(&ids).is_none(),
            "non-sequential numbers were taken for a gutter"
        );
        // Numbers glued to their text.
        let glued: String = (1..=20).map(|i| format!("{i}files changed\n")).collect();
        assert!(
            split(&glued).is_none(),
            "a number with no separator was taken for a gutter"
        );
    }

    /// One unnumbered line is enough to decline — a partial strip would eat
    /// content from the lines that DO have one while leaving the rest.
    #[test]
    fn one_line_without_a_number_declines_the_whole_scan() {
        let mut t = numbered(30, 1);
        t.push_str("a trailing note with no number\n");
        assert!(split(&t).is_none());
    }

    /// Too few lines is an ordered list, not `cat -n`.
    #[test]
    fn a_handful_of_numbered_lines_is_a_list() {
        assert!(split(&numbered(MIN_LINES - 1, 1)).is_none());
        assert!(split(&numbered(MIN_LINES, 1)).is_some());
    }

    #[test]
    fn the_note_names_the_range_it_removed() {
        let n = note(&Gutter {
            first: 1,
            last: 187,
            lines: 187,
        });
        assert!(n.contains("1-187") && n.contains("187 lines"), "{n}");
    }
}
