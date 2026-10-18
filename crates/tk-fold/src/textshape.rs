//! Is this buffer plain text? The question every magic-byte detector needs
//! to ask before it answers, and none of them did.
//!
//! A magic signature made of printable ASCII is indistinguishable from a
//! sentence that happens to start the same way. `MAC ` is Monkey's Audio and
//! also the first four bytes of `MAC host=127.0.0.1`; `MThd` is a MIDI header
//! and also `MThd` in a hex table; `SQLite format 3` is a database header and
//! also a phrase someone writes in a note. Measured on the shipped 1.1.6
//! binary, seven signatures fired on ordinary text — and `MAC ` did not merely
//! hide the line, it sent it to local whisper transcription.
//!
//! The discriminator is not the signature, it is everything after it. Audio,
//! video, images and archives reach a NUL byte or an invalid UTF-8 sequence
//! within a few dozen bytes; prose does not. So a detector that matched an
//! ASCII signature asks here whether the rest of the buffer agrees, and drops
//! the verdict when it does not.
//!
//! This is deliberately NOT symmetric with the density heuristic in
//! `disclose::binary`: that one asks "is this binary?" and answers yes on ≥5 %
//! control bytes. This one asks "is this unambiguously text?" and answers yes
//! only well inside the safe margin, because its job is to VETO a positive
//! detection, and a wrong veto hands raw bytes to a text compactor.

/// How much of the buffer to judge. Matches the density-heuristic sample in
/// `disclose::binary` so the two never disagree about what they looked at.
const SAMPLE: usize = 4096;

/// True when `bytes` are unambiguously plain text: valid UTF-8 over the
/// sampled prefix, no NUL byte, and under 1 % control characters.
///
/// Under-1 % rather than the 5 % the binary side uses: this predicate exists
/// to overrule a magic match, so it must be the stricter of the two. A file
/// that sits between the two thresholds is claimed by neither, which leaves
/// the magic verdict standing — the pre-existing behaviour.
pub fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let end = truncation_safe_end(bytes, bytes.len().min(SAMPLE));
    let sample = &bytes[..end];
    if sample.is_empty() {
        return false;
    }
    if std::str::from_utf8(sample).is_err() {
        return false;
    }
    if sample.contains(&0) {
        return false;
    }
    let census = control_census(sample);
    census.payload > 0 && census.control * 100 < census.payload
}

/// How many control bytes a buffer carries, once terminal escape sequences are
/// set aside — and how many bytes are left after setting them aside.
///
/// The two predicates that judge "binary" both counted `ESC` (0x1B) as a
/// control byte, and coloured tool output is nothing but `ESC`. Measured
/// 2026-09-01 on this machine: `grep -rn --color=always 'fn ' <file>` is
/// **11.9 %** escape bytes, `cargo test --color=always` 4.1 %, `git -c
/// color.ui=always log --graph` 2.9 %. The density heuristic calls anything at
/// or above 5 % binary, so a 558-byte grep result — ordinary readable text —
/// was handed to the agent as `(Binary content — not shown as text)` with the
/// bytes stashed away. The agent's only move is to run the command again
/// without colour, which is the turn this product exists to save.
///
/// The stripping is not the fix on its own: `strip_ansi` already runs on the
/// text path, and it runs AFTER this decision. An escape sequence is
/// presentation, so it is neither evidence of binary nor part of what the
/// question is about — it comes out of BOTH the numerator and the denominator,
/// which is what keeps a buffer that is 90 % escapes and 10 % NUL binary.
///
/// An `ESC` that does not open a well-formed sequence still counts as a control
/// byte: a real binary full of stray 0x1B stays binary.
pub fn control_census(sample: &[u8]) -> ControlCensus {
    let mut control = 0usize;
    let mut escape = 0usize;
    let mut i = 0usize;
    while i < sample.len() {
        let b = sample[i];
        if b == 0x1B && i + 1 < sample.len() {
            if let Some(len) = escape_sequence_len(&sample[i..]) {
                escape += len;
                i += len;
                continue;
            }
        }
        if b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t' {
            control += 1;
        }
        i += 1;
    }
    ControlCensus {
        control,
        payload: sample.len() - escape,
    }
}

/// The two numbers [`control_census`] answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlCensus {
    /// Control bytes that are NOT part of a terminal escape sequence.
    pub control: usize,
    /// Bytes left once the escape sequences are set aside.
    pub payload: usize,
}

/// Length of the terminal escape sequence starting at `s[0]`, or `None` when
/// what follows the `ESC` is not one.
///
/// Three shapes, which is what colour and cursor movement actually use: CSI
/// (`ESC [` params intermediates final), OSC (`ESC ]` … terminated by BEL or
/// `ESC \`), and the two-byte escapes (`ESC` followed by `@`..`_`). A sequence
/// cut in half by the end of the sample does not match, so its `ESC` counts as
/// a control byte — one byte, at one place in the buffer.
fn escape_sequence_len(s: &[u8]) -> Option<usize> {
    if s.len() < 2 || s[0] != 0x1B {
        return None;
    }
    match s[1] {
        b'[' => {
            let mut j = 2;
            // Parameter bytes 0x30-0x3F, then intermediate bytes 0x20-0x2F.
            while j < s.len() && (0x20..=0x3F).contains(&s[j]) {
                j += 1;
            }
            if j < s.len() && (0x40..=0x7E).contains(&s[j]) {
                Some(j + 1)
            } else {
                None
            }
        }
        b']' => {
            let mut j = 2;
            while j < s.len() {
                if s[j] == 0x07 {
                    return Some(j + 1);
                }
                if s[j] == 0x1B && j + 1 < s.len() && s[j + 1] == b'\\' {
                    return Some(j + 2);
                }
                j += 1;
            }
            None
        }
        c if (0x40..=0x5F).contains(&c) => Some(2),
        _ => None,
    }
}

/// Back off `end` to the nearest UTF-8 character boundary at or below it, so
/// slicing a multi-byte character in half is never read as invalid UTF-8.
///
/// The byte-as-char class has cost this repo six compactors already. A 4 KB
/// cut landing mid-character would make every large UTF-8 document answer
/// "not text" — the exact failure this module exists to prevent, arriving
/// through the module itself.
fn truncation_safe_end(bytes: &[u8], mut end: usize) -> usize {
    if end >= bytes.len() {
        return bytes.len();
    }
    // A UTF-8 continuation byte is 0b10xxxxxx. Walk back at most 3 of them.
    let mut backed = 0;
    while end > 0 && backed < 4 && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
        backed += 1;
    }
    end
}

/// Formats whose bytes are legitimately printable ASCII, so
/// [`looks_like_text`] cannot be used to overrule them.
///
/// RTF and PostScript are text files by construction — every byte of a valid
/// `.rtf` can be ASCII. Vetoing their detection would break the document
/// extraction path that reads them, which is a real capability, in order to
/// fix a false positive on prose that quotes `{\rtf1` or `%!PS`. The false
/// positive is the cheaper loss, so these keep their verdict.
pub fn is_text_encoded_format(ext: &str) -> bool {
    matches!(ext, "rtf" | "ps" | "eps" | "ai")
}

/// **A preview must not end inside a construct.**
///
/// A folded section's preview QUOTES one of its lines, and that line can
/// be a figure. Cut at one hundred and sixty characters without looking at what it
/// cut, it returned a bracket that never closes:
///
/// ```text
/// ## …scandale national⟨9⟩ (3 lines · "[image: Un hélicoptère de la Direction…")
/// ```
///
/// The `[image:` opens, the cut falls in the middle of the alt text, and the
/// reader — human or machine — sees an image whose description swallows what
/// follows. MEASURED 2026-09-09 on the 39 pages of the corpus: **10 orphan
/// brackets on 3 pages**, seven of them on `franceinfo.fr`.
///
/// It is the rule "the cut falls on a SEPARATOR, never in the
/// middle" applied to markup rather than words, together with "a
/// fold does not cross its neighbour's structure". We move back to before the
/// opening bracket left alone; if nothing useful remains, the caller keeps
/// its empty preview and writes `(N lines)`, which it already knows how to do.
pub fn cut_before_an_unclosed_bracket(s: &str) -> &str {
    // Brackets AND parentheses. Both open a construct that the cut can
    // slice, and on this route they nest: a section's preview
    // is written `(N lines · "…")` and may contain `[image: …]`. Backing off before
    // the bracket alone left `(3 lines · "` dangling — shorter, still
    // open. We back off before the FIRST of the two that is still open.
    let mut depth = 0usize;
    let mut first_unclosed: Option<usize> = None;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '(' => {
                if depth == 0 {
                    first_unclosed = Some(i);
                }
                depth += 1;
            }
            ']' | ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    first_unclosed = None;
                }
            }
            _ => {}
        }
    }
    match first_unclosed {
        Some(i) => s[..i].trim_end(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(test)]
    mod a_cut_never_lands_inside_a_construct {
        use super::super::cut_before_an_unclosed_bracket as cut;

        /// The real case: a section's preview quotes a figure, and the cut falls
        /// in the middle of the alt text. The bracket opened and never
        /// closed — seven times on `franceinfo.fr`.
        #[test]
        fn an_image_bracket_cut_in_half_is_dropped() {
            assert_eq!(cut("avant [image: une descrip"), "avant");
            // Closed, it stays: the rule does not eat what is complete.
            assert_eq!(cut("avant [image: x] après"), "avant [image: x] après");
        }

        /// Parentheses count too, and this is measured: backing off before the
        /// bracket alone left `(3 lines · "` dangling — shorter, still open.
        #[test]
        fn an_unclosed_parenthesis_counts_too() {
            assert_eq!(
                cut("### Titre (3 lines · \"[image: Des hommes"),
                "### Titre"
            );
            assert_eq!(
                cut("#### func ServeTLS⟨55⟩ added in go1.9 (13 lines"),
                "#### func ServeTLS⟨55⟩ added in go1.9"
            );
        }

        /// The FIRST construct still open, not the last: backing off before the
        /// inner bracket would keep the parenthesis around it.
        #[test]
        fn the_outermost_unclosed_one_wins() {
            assert_eq!(cut("t (a · \"[image: x"), "t");
        }

        /// The bound on both sides: what is balanced passes through intact,
        /// nested included, otherwise the rule would eat correct previews.
        #[test]
        fn balanced_text_is_untouched() {
            assert_eq!(cut("a (b [c] d) e"), "a (b [c] d) e");
            assert_eq!(cut("rien à faire ici"), "rien à faire ici");
            assert_eq!(cut(""), "");
        }

        /// An ORPHAN closing must not make what follows fragile: a lone `]`
        /// comes from a text, not from a construct we would have opened.
        #[test]
        fn a_stray_closer_does_not_underflow() {
            assert_eq!(cut("a] b [c"), "a] b");
        }
    }

    #[test]
    fn prose_starting_with_an_ascii_magic_is_text() {
        for s in [
            "MAC host=127.0.0.1 port=2222 user=macos\n",
            "MThd midi header layout notes\n",
            "MSCF cabinet format, first field\n",
            "8BPS is the Photoshop signature\n",
            "SQLite format 3 is 16 bytes with a NUL\n",
            "%!PS-Adobe-3.0 appears in every eps\n",
            "ID3 tagging guide for the archive\n",
            "RIFF containers hold WAVE or AVI\n",
            "FORM is the IFF chunk name\n",
            "fLaC is the FLAC stream marker\n",
            "OggS begins every Ogg page\n",
            ".snd is the Sun audio header\n",
        ] {
            assert!(looks_like_text(s.as_bytes()), "should be text: {s:?}");
        }
    }

    #[test]
    fn real_binary_headers_are_not_text() {
        // PNG, ELF, gzip, and a Monkey's Audio header with its version field.
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x01\x00";
        let elf = b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        let gz = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\xed\x9d";
        let ape = b"MAC \x96\x0f\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        for b in [&png[..], &elf[..], &gz[..], &ape[..]] {
            assert!(!looks_like_text(b), "should be binary: {b:?}");
        }
    }

    #[test]
    fn empty_is_not_text() {
        assert!(!looks_like_text(b""));
    }

    #[test]
    fn a_nul_anywhere_in_the_sample_disqualifies() {
        let mut v = b"MAC host=127.0.0.1".to_vec();
        v.push(0);
        v.extend_from_slice(b" and more text after the nul");
        assert!(!looks_like_text(&v));
    }

    #[test]
    fn a_multibyte_character_cut_by_the_sample_is_still_text() {
        // Fill past SAMPLE with 3-byte characters so the cut lands mid-char
        // for at least one of the offsets exercised here.
        for pad in 0..3 {
            let mut s = "MAC ".to_string();
            s.push_str(&" ".repeat(pad));
            while s.len() < SAMPLE + 16 {
                s.push('あ');
            }
            assert!(
                looks_like_text(s.as_bytes()),
                "pad={pad} should stay text across the sample cut"
            );
        }
    }

    #[test]
    fn control_characters_above_one_percent_are_not_claimed() {
        // 200 bytes, 4 control chars = 2 % → not claimed as text, so a magic
        // verdict on such a buffer still stands.
        let mut v = b"MAC ".to_vec();
        v.extend(std::iter::repeat_n(b'a', 196));
        for i in 0..4 {
            v[10 + i * 10] = 0x01;
        }
        assert!(!looks_like_text(&v));
    }

    #[test]
    fn text_encoded_formats_are_exempt() {
        assert!(is_text_encoded_format("rtf"));
        assert!(is_text_encoded_format("ps"));
        assert!(!is_text_encoded_format("ape"));
        assert!(!is_text_encoded_format("mid"));
    }

    /// The finding: coloured tool output is text, and both predicates used to
    /// say otherwise because they counted `ESC` as a control byte. The
    /// densities are the ones measured on this machine — `grep
    /// --color=always` is the worst at 11.9 %, well past the 5 % the binary
    /// side calls binary.
    #[test]
    fn coloured_tool_output_is_text_at_every_density_we_measured() {
        let line = |sgr: &str, body: &str| format!("\x1b[{sgr}m{body}\x1b[m");
        // A grep hit: filename, line number, separators and the match, each in
        // its own colour. This is the 11.9 % shape.
        let mut grep = String::new();
        for i in 0..40 {
            grep.push_str(&line("32", &format!("crates/x/{i}.rs")));
            grep.push_str(&line("36", ":"));
            grep.push_str(&format!("{i}"));
            grep.push_str(&line("36", ":"));
            grep.push_str(&line("01;31", "fn "));
            grep.push_str("do_something(&self) {\n");
        }
        assert!(
            looks_like_text(grep.as_bytes()),
            "a coloured grep result is text"
        );
        let census = control_census(grep.as_bytes());
        assert_eq!(
            census.control, 0,
            "colour is presentation; nothing here is a control byte"
        );
        assert!(
            census.payload < grep.len(),
            "the escape bytes must leave the denominator too"
        );
    }

    /// The other direction, which is what keeps the change honest: setting
    /// escape sequences aside must not launder a buffer that is genuinely
    /// binary. A payload of NULs stays binary however much colour is wrapped
    /// around it.
    #[test]
    fn escape_sequences_do_not_launder_real_binary() {
        let mut v: Vec<u8> = Vec::new();
        for _ in 0..200 {
            v.extend_from_slice(b"\x1b[32m");
            v.push(0x01);
            v.push(0x02);
            v.extend_from_slice(b"ab");
        }
        let census = control_census(&v);
        assert_eq!(
            census.control, 400,
            "the non-escape control bytes still count"
        );
        assert!(
            census.control * 100 / census.payload >= 5,
            "a payload of control bytes is still binary: {census:?}"
        );
        assert!(!looks_like_text(&v));

        // A NUL is disqualifying on its own, escaped or not.
        assert!(!looks_like_text(b"\x1b[32mhello\x1b[m\0world"));
    }

    /// A stray `ESC` that opens nothing is a control byte, not free presentation
    /// — otherwise a binary full of 0x1B would read as text.
    #[test]
    fn a_stray_escape_is_not_a_sequence() {
        let mut v = vec![0x1Bu8; 100];
        v.extend_from_slice(b"abcdefghij");
        let census = control_census(&v);
        assert_eq!(
            census.control, 100,
            "an ESC that opens no sequence counts: {census:?}"
        );
        assert_eq!(census.payload, 110, "and it stays in the denominator");
        assert!(!looks_like_text(&v));
    }

    /// The three sequence shapes colour and cursor movement actually use, plus
    /// the one that is cut in half by the end of the sample.
    #[test]
    fn the_sequence_shapes_are_recognised_and_a_cut_one_is_not() {
        for (seq, what) in [
            ("\x1b[0m", "SGR reset"),
            ("\x1b[01;31m", "SGR with parameters"),
            ("\x1b[2J", "erase display"),
            ("\x1b]0;a title\x07", "OSC ended by BEL"),
            ("\x1b]8;;http://x\x1b\\", "OSC ended by ST"),
            ("\x1bM", "two-byte escape"),
        ] {
            let body = format!("{seq}text");
            let census = control_census(body.as_bytes());
            assert_eq!(census.control, 0, "{what} must be recognised: {census:?}");
            assert_eq!(
                census.payload, 4,
                "{what} must leave only `text` in the payload: {census:?}"
            );
        }
        // Cut by the end of the buffer: nothing to match, so the ESC counts.
        let census = control_census(b"\x1b[01;3");
        assert_eq!(
            census.control, 1,
            "a truncated sequence is one control byte"
        );
    }

    /// Nothing but escape sequences is a screen clear, not a binary — and a
    /// zero payload must not divide by zero on either side.
    #[test]
    fn a_buffer_of_pure_escapes_divides_by_nothing() {
        let v = b"\x1b[2J\x1b[H\x1b[0m".repeat(20);
        let census = control_census(&v);
        assert_eq!(census.payload, 0);
        assert_eq!(census.control, 0);
        assert!(
            !looks_like_text(&v),
            "there is no text here to affirm, but it must not panic"
        );
        // "and it is not a binary either" is checked one layer
        // higher, where `disclose` is reachable: see the `fold` target,
        // `a_buffer_of_pure_escapes_is_not_binary_either`.
    }
}
