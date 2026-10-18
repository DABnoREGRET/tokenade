//! Two cleanups on extracted web text, applied at the pipeline's single exit.
//!
//! Both fix bytes that a READER never sees but an agent always pays for:
//!
//!   * invisible Unicode — zero-width and bidi controls that render as nothing
//!     but each cost a token (worse: they are multi-byte, so a zero-width space
//!     injected between every letter — a known anti-scraping trick — roughly
//!     triples a paragraph's token count);
//!   * CSS letter-spacing — a heading set with `letter-spacing` or with a
//!     literal space between every glyph extracts as `S t a r t   H e r e`,
//!     which tokenises as nine tokens instead of two.
//!
//! Both passes are deliberately CONSERVATIVE. Destroying real content is a far
//! worse outcome than paying for a few stray characters, so where the safe rule
//! and the thorough rule disagree, the safe rule wins and the comment says why.

/// Characters that are always safe to drop: they render as nothing and carry no
/// meaning a reader can act on.
///
///   * U+200B zero-width space, U+2060 word joiner, U+FEFF BOM / zero-width
///     no-break space — pure layout hints;
///   * U+00AD soft hyphen — marks an OPTIONAL break point, so `in\u{AD}visible`
///     is the single word `invisible`. It must be removed rather than turned
///     into a hyphen, or the word is silently misspelt;
///   * U+200E/200F and U+202A–U+202E and U+2066–U+2069 — bidi embedding and
///     isolate controls. They change nothing about which characters are there,
///     only how a bidirectional renderer orders them; a model reads the string
///     in logical order regardless.
///
/// U+200C (ZWNJ) and U+200D (ZWJ) are NOT in this list — see
/// [`joiner_is_decorative`].
fn is_always_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'
            | '\u{2060}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// Is this ZWNJ/ZWJ decorative, i.e. safe to drop?
///
/// These two are the only members of the family with real semantics, and the
/// scripts that need them are exactly the ones we would corrupt beyond repair:
///
///   * Persian/Arabic ZWNJ separates words written without a space (`می‌رود`) —
///     dropping it merges two words into one that does not exist;
///   * Indic ZWJ/ZWNJ selects between a conjunct ligature and its split form,
///     changing which letters the reader sees;
///   * emoji ZWJ sequences are BUILT from it — family, profession and
///     skin-tone-plus-role emoji are `person ZWJ role`, and removing the joiner
///     turns one glyph into two unrelated ones.
///
/// So the rule is inverted from "strip unless dangerous" to "keep unless
/// provably pointless": drop the joiner only when BOTH neighbours are ASCII.
/// Nothing in Arabic, any Indic script, or any pictographic sequence is ASCII,
/// so this cannot reach a case where the joiner means something — while still
/// catching the case that actually costs us, joiners sprinkled through English
/// prose by a CMS or an obfuscator. A joiner at the very start or end of the
/// text has no second neighbour to prove anything about, so it is kept.
fn joiner_is_decorative(prev: Option<char>, next: Option<char>) -> bool {
    matches!((prev, next), (Some(p), Some(n)) if p.is_ascii() && n.is_ascii())
}

/// Drop invisible characters. See [`is_always_invisible`] and
/// [`joiner_is_decorative`] for what is dropped and what is deliberately not.
pub fn strip_invisible(s: &str) -> String {
    // Overwhelmingly the text is clean; scanning once and handing back the
    // input untouched avoids allocating a copy of every page we distil.
    if !s
        .chars()
        .any(|c| is_always_invisible(c) || matches!(c, '\u{200C}' | '\u{200D}'))
    {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if is_always_invisible(c) {
            continue;
        }
        if matches!(c, '\u{200C}' | '\u{200D}') {
            // Neighbours are taken from the ORIGINAL text, not from what has
            // been emitted so far: a run like `a\u{200B}\u{200D}b` should still
            // see `a` on the left even though the ZWSP before it went away.
            let prev = chars[..i]
                .iter()
                .rev()
                .find(|p| !is_always_invisible(**p))
                .copied();
            let next = chars[i + 1..]
                .iter()
                .find(|n| !is_always_invisible(**n))
                .copied();
            if joiner_is_decorative(prev, next) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// A run shorter than this is not evidence of letter-spacing. `a b c` appears in
/// ordinary prose and in list items; four consecutive single letters separated
/// by single spaces essentially does not, outside of spaced-out display type.
const MIN_RUN: usize = 4;

/// Can this token be a letter of a letter-spaced word?
///
/// Latin letters only, and no digits. Two exclusions bought deliberately:
///
///   * CJK — `你 好 世 界` is spaced Chinese where the spaces may be the
///     author's; joining it changes the text, and CJK does not get set with
///     CSS letter-spacing in the first place because every glyph is already
///     full-width;
///   * digits — `1 2 3 4` is a table row far more often than it is a
///     letter-spaced year, and merging it into `1234` invents a number.
///
/// The upper bound covers Latin-1 Supplement and Latin Extended-A/B, so an
/// accented display heading (`É T É`) still rejoins.
fn is_spaceable_letter(tok: &str) -> bool {
    let mut it = tok.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => c.is_alphabetic() && (c as u32) < 0x0250,
        _ => false,
    }
}

/// Rejoin CSS letter-spaced text: `S t a r t  H e r e` → `Start Here`.
///
/// One space between single letters is intra-word spacing; TWO OR MORE is the
/// real word boundary. Note that an HTML source rarely gets to use that rule:
/// CSS collapses any run of whitespace to a single space, so `S t a r t  H e r
/// e` reaches us as a single undifferentiated run and rejoins to `StartHere`.
/// The two-space rule earns its keep on the sources that preserve spacing —
/// feeds, JSON outlines, plain text, `<pre>` — and costs nothing on HTML.
/// Anything that is not a single Latin letter ends the run, so
/// `J. R. R. Tolkien` (two-character tokens), `n = 1` (an operator) and a
/// pipe-delimited table row are all left alone by construction.
pub fn rejoin_letter_spacing(s: &str) -> String {
    // Fenced code is off limits: a rejoin inside a code block would corrupt
    // something the agent is expected to run or copy verbatim, and short
    // identifiers on a line are common there.
    let mut in_fence = false;
    let mut out = String::with_capacity(s.len());
    for (i, raw_line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // Split the CRLF carriage return off before tokenising. Left attached
        // it rides along on the LAST token of the line, and
        // `is_spaceable_letter` takes exactly one char — so `o\r` is rejected,
        // the run stops one letter short, and `H e l l o` rejoins as `Hell o`.
        // That severed every final letter on Windows, where tool output is
        // CRLF, and nowhere else. `split('\n')` is deliberate here (it must
        // preserve the exact line structure, which `lines()` would not), so
        // the carriage return is ours to handle.
        let (line, cr) = match raw_line.strip_suffix('\r') {
            Some(l) => (l, "\r"),
            None => (raw_line, ""),
        };
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
        } else if in_fence {
            out.push_str(line);
        } else {
            out.push_str(&rejoin_line(line));
        }
        out.push_str(cr);
    }
    out
}

fn rejoin_line(line: &str) -> String {
    // (token, number of spaces that preceded it). Leading indentation is the
    // gap of the first token, so it survives untouched.
    let mut toks: Vec<(&str, usize)> = Vec::new();
    let mut gap = 0usize;
    let mut start: Option<usize> = None;
    for (i, c) in line.char_indices() {
        if c == ' ' {
            if let Some(st) = start.take() {
                toks.push((&line[st..i], gap));
                gap = 0;
            }
            gap += 1;
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        toks.push((&line[st..], gap));
    } else if !toks.is_empty() || gap > 0 {
        // Trailing spaces (markdown hard break) must not be eaten.
        let mut out = rebuild(line, &toks);
        out.push_str(&" ".repeat(gap));
        return out;
    }
    rebuild(line, &toks)
}

fn rebuild(line: &str, toks: &[(&str, usize)]) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;
    while i < toks.len() {
        if !is_spaceable_letter(toks[i].0) {
            out.push_str(&" ".repeat(toks[i].1));
            out.push_str(toks[i].0);
            i += 1;
            continue;
        }
        // Extend over every following single letter, whatever the gap width —
        // the gap only decides whether a space is emitted, not whether the run
        // continues, so `S t a r t  H e r e` is ONE run of nine letters.
        let mut j = i + 1;
        while j < toks.len() && is_spaceable_letter(toks[j].0) && toks[j].1 >= 1 {
            j += 1;
        }
        if j - i < MIN_RUN {
            out.push_str(&" ".repeat(toks[i].1));
            out.push_str(toks[i].0);
            i += 1;
            continue;
        }
        out.push_str(&" ".repeat(toks[i].1));
        out.push_str(toks[i].0);
        for t in &toks[i + 1..j] {
            // Two or more spaces was the word boundary; collapse it to one.
            if t.1 >= 2 {
                out.push(' ');
            }
            out.push_str(t.0);
        }
        i = j;
    }
    out
}

/// Both passes, in the order that matters: invisibles first, because a
/// zero-width space between the letters of `S\u{200B} t\u{200B} a…` would
/// otherwise hide the letter-spacing from the second pass.
pub fn clean(s: &str) -> String {
    rejoin_letter_spacing(&strip_invisible(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_purely_decorative_invisibles() {
        let s = "a\u{200B}b\u{2060}c\u{FEFF}d\u{200E}e\u{202A}f\u{202E}g\u{2066}h\u{2069}i";
        assert_eq!(strip_invisible(s), "abcdefghi");
    }

    /// A soft hyphen is an optional break point, not a hyphen: keeping it as `-`
    /// would misspell the word, and keeping it at all costs a token per break.
    #[test]
    fn soft_hyphen_is_removed_not_turned_into_a_hyphen() {
        assert_eq!(strip_invisible("in\u{00AD}visible"), "invisible");
        assert_eq!(
            strip_invisible("Sil\u{00AD}ben\u{00AD}tren\u{00AD}nung"),
            "Silbentrennung"
        );
    }

    /// The negative that matters most: these joiners ARE the text in Arabic,
    /// Indic scripts and emoji sequences.
    #[test]
    fn joiners_survive_where_they_carry_meaning() {
        // Persian ZWNJ between two Arabic-script letters.
        let fa = "می\u{200C}رود";
        assert_eq!(strip_invisible(fa), fa);

        // Devanagari ZWJ.
        let hi = "क\u{200D}ष";
        assert_eq!(strip_invisible(hi), hi);

        // Emoji ZWJ sequence: family, and a profession with a skin tone.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(strip_invisible(family), family);
        let dev = "\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}";
        assert_eq!(strip_invisible(dev), dev);

        // Mixed: an ASCII neighbour on one side only is still not proof.
        assert_eq!(strip_invisible("x\u{200D}\u{1F467}"), "x\u{200D}\u{1F467}");
        // At a boundary there is no second neighbour at all.
        assert_eq!(strip_invisible("\u{200D}abc"), "\u{200D}abc");
    }

    /// Between two ASCII characters no script needs a joiner, so it is noise.
    #[test]
    fn joiners_between_ascii_are_dropped() {
        assert_eq!(strip_invisible("cli\u{200C}ck he\u{200D}re"), "click here");
    }

    #[test]
    fn rejoins_letter_spaced_headings() {
        assert_eq!(rejoin_letter_spacing("S t a r t"), "Start");
        assert_eq!(rejoin_letter_spacing("S t a r t  H e r e"), "Start Here");
        // Three spaces is still one word boundary, not three.
        assert_eq!(rejoin_letter_spacing("S t a r t   H e r e"), "Start Here");
        // Accented display type: the run is Latin, not just ASCII.
        assert_eq!(rejoin_letter_spacing("R É S U M É"), "RÉSUMÉ");
        // Digits are not run members, so they keep their own spacing.
        assert_eq!(
            rejoin_letter_spacing("P R I N T E M P S  2 0 2 5"),
            "PRINTEMPS  2 0 2 5"
        );
    }

    #[test]
    fn rejoin_keeps_surrounding_text_and_indentation() {
        assert_eq!(
            rejoin_letter_spacing("## W E L C O M E to the site"),
            "## WELCOME to the site"
        );
        assert_eq!(rejoin_letter_spacing("  - S t a r t"), "  - Start");
        assert_eq!(
            rejoin_letter_spacing("line one\nS t a r t\nlast"),
            "line one\nStart\nlast"
        );
    }

    /// The false positives that would destroy real content.
    #[test]
    fn rejoin_leaves_real_content_alone() {
        for s in [
            "J. R. R. Tolkien",    // two-character tokens, never a run
            "n = 1",               // an operator ends the run
            "a b c",               // below MIN_RUN
            "x + y = z",           //
            "你 好 世 界",         // CJK is excluded outright
            "1 2 3 4 5",           // digits are excluded outright
            "| a | b | c | d |",   // table row, pipes break the run
            "- a\n- b\n- c\n- d",  // one-letter list items, one per line
            "The quick brown fox", //
            "A B",                 //
        ] {
            assert_eq!(rejoin_letter_spacing(s), s, "mangled: {s:?}");
        }
    }

    #[test]
    fn fenced_code_is_never_rejoined() {
        let src = "text\n```\na b c d e\n```\nS t a r t";
        assert_eq!(
            rejoin_letter_spacing(src),
            "text\n```\na b c d e\n```\nStart"
        );
    }

    /// A markdown hard break is two trailing spaces; eating them would join two
    /// lines that the page kept apart.
    #[test]
    fn trailing_spaces_survive() {
        assert_eq!(rejoin_letter_spacing("line  "), "line  ");
        assert_eq!(rejoin_letter_spacing(""), "");
        assert_eq!(rejoin_letter_spacing("   "), "   ");
    }

    /// Invisibles must go first, or they hide the letter-spacing from pass two.
    #[test]
    fn clean_runs_both_passes_in_order() {
        assert_eq!(clean("S\u{200B} t\u{200B} a\u{200B} r\u{200B} t"), "Start");
    }

    /// The rejoin must not depend on the platform that produced the text.
    ///
    /// On Windows every tool output is CRLF, and the carriage return used to
    /// ride along on the last token of each line. `is_spaceable_letter` takes
    /// exactly one char, so `o\r` was rejected and the run stopped one letter
    /// short: `H e l l o` came back as `Hell o`. Silent, Windows-only, and it
    /// corrupted the very text this pass exists to repair.
    #[test]
    fn crlf_rejoins_exactly_like_lf() {
        let lf = "S t a r t  H e r e\nplain line\nM o r e  T e x t";
        let crlf = lf.replace('\n', "\r\n");

        let got_lf = rejoin_letter_spacing(lf);
        let got_crlf = rejoin_letter_spacing(&crlf);

        assert_eq!(got_lf, "Start Here\nplain line\nMore Text");
        assert_eq!(
            got_crlf.replace("\r\n", "\n"),
            got_lf,
            "CRLF input rejoined differently from LF: {got_crlf:?}"
        );
        assert!(
            got_crlf.contains("\r\n"),
            "the carriage returns were eaten: {got_crlf:?}"
        );
    }

    /// A fenced block is copied verbatim, carriage return included.
    #[test]
    fn crlf_inside_a_fence_survives_untouched() {
        let src = "```\r\nS t a r t\r\n```\r\ndone";
        assert_eq!(rejoin_letter_spacing(src), src);
    }
}
