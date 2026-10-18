//! ANSI escapes, and the only function that removes them.
//!
//! The full grammar, not just the coloured half: CSI, OSC, DCS,
//! APC, PM, SOS, RIS and the character-set switches. The comments
//! below carry the measurement for each — these are the escapes that
//! DO something (OSC 52 writes the reader's clipboard, RIS wipes
//! its history) and they used to pass through whole.
//!
//! This code lived in `filter.rs`, the command dispatcher, and nine
//! FORMAT modules (`ansible`, `cargobench`, `containerlogs`, `gitlog`,
//! `kubectl`, `logs`, `pkginstall`, `searchout`, `tabular`) had to climb
//! into the commands layer to remove a terminal escape —
//! 10 call sites. Removing an ANSI sequence requires knowing neither a
//! command nor a format.
//!
//! `user_filters` carried a SECOND copy, the same CSI pattern
//! verbatim, and it never received OSC nor the rest of the grammar:
//! a rule proven on one member of a family and not on the others, the
//! shape that has cost this repo the most. It calls this one now.

crate::regexes::lazy_regex!(fn ansi_re() = r"\x1b\[[0-9;?]*[ -/]*[@-~]");

// OSC — `ESC ]` … terminated by BEL or ST (`ESC \`). NOT matched by the CSI
// pattern above, and these are the escapes that DO something rather than
// colour something: OSC 52 writes the reader's system clipboard, OSC 0/2
// rewrites the window title, OSC 8 turns arbitrary text into a link to
// anywhere. Measured 2026-09-06 on the published binary: a command emitting
// `ESC ] 52 ; c ; <base64> BEL` came back through `tokenade wrap` byte for
// byte, while a CSI colour on the same path was stripped — so a function
// documented as stripping "ANSI escape sequences" removed the harmless half
// and forwarded the capable half.
crate::regexes::lazy_regex!(fn osc_re() = r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)");

// The rest of the ESC grammar a terminal acts on. OSC was fixed first and its
// siblings were left, which is the shape that has cost this codebase most
// often — a rule proven on one member of a family and not on the others.
// Measured 2026-09-06, all six passing `tokenade wrap` untouched:
//
//   * DCS `ESC P … ST`   — device control, sixel graphics
//   * APC `ESC _ … ST`   — the kitty graphics protocol
//   * PM  `ESC ^ … ST`   — privacy message
//   * SOS `ESC X … ST`
//   * RIS `ESC c`        — **full terminal reset**, clears the scrollback
//   * charset `ESC ( 0`  — switches to line-drawing, so every following
//                          character renders as box garbage until reset
//
// The two-byte forms are listed EXPLICITLY rather than matched as
// `ESC <any letter>`: a stray ESC in ordinary content would otherwise eat the
// character after it. Only the escapes that DO something are named.
crate::regexes::lazy_regex!(
    fn esc_other_re() = r"\x1b(?:[P_^X][^\x1b]*(?:\x1b\\|\x07)|[()*+][0-9A-Za-z]|[cDEHMNO78=><])"
);

#[cfg(test)]
mod osc_stripping {
    use super::strip_ansi;

    /// The escapes that DO something must not survive to a terminal.
    ///
    /// `strip_ansi` matched CSI only, so a function documented as stripping
    /// "ANSI escape sequences" removed the half that colours text and forwarded
    /// the half that acts: OSC 52 writes the reader's system clipboard, OSC 0/2
    /// rewrites the window title, OSC 8 points arbitrary text at any URL.
    /// Measured 2026-09-06 on the published binary through `tokenade wrap`: an
    /// OSC 52 clipboard write came back byte for byte while a CSI colour on the
    /// same path was stripped.
    /// OSC's siblings, checked because OSC alone was fixed first.
    ///
    /// Every one of these passed `tokenade wrap` untouched on the published
    /// binary. Two of them are not cosmetic: `ESC c` resets the terminal and
    /// clears the scrollback, and `ESC ( 0` switches to the line-drawing
    /// charset so everything after it renders as box characters until
    /// something resets it.
    #[test]
    fn oscs_siblings_never_reach_the_reader_either() {
        // ST-terminated string families: the payload goes with the escape.
        assert_eq!(strip_ansi("a\x1bPq#0;2;0;0;0#0~~@@\x1b\\b"), "ab", "DCS");
        assert_eq!(strip_ansi("a\x1b_Gf=100,a=T;AAAA\x1b\\b"), "ab", "APC");
        assert_eq!(strip_ansi("a\x1b^hidden\x1b\\b"), "ab", "PM");
        assert_eq!(strip_ansi("a\x1bXsos\x1b\\b"), "ab", "SOS");

        // Two-byte escapes that ACT.
        assert_eq!(strip_ansi("a\x1bcb"), "ab", "RIS resets the terminal");
        assert_eq!(strip_ansi("a\x1b7b\x1b8c"), "abc", "cursor save/restore");

        // Charset designator: the escape goes, its letters stay as letters.
        assert_eq!(strip_ansi("a\x1b(0lqkb"), "alqkb", "charset shift");

        // Anti-vacuity. The two-byte forms are named explicitly precisely so a
        // stray ESC cannot eat the character after it, and so ordinary text
        // containing these letters is never touched.
        assert_eq!(
            strip_ansi("a\x1bzb"),
            "a\x1bzb",
            "an unlisted escape is left alone"
        );
        assert_eq!(
            strip_ansi("cost: 12 USD, ratio 3:1, (paren) and json]"),
            "cost: 12 USD, ratio 3:1, (paren) and json]"
        );
        assert_eq!(
            strip_ansi("\x1b[31mRED\x1b[0m ok"),
            "RED ok",
            "CSI still goes"
        );
        assert_eq!(
            strip_ansi("50%\r100% done"),
            "100% done",
            "redraws still collapse"
        );
    }

    #[test]
    fn an_osc_sequence_never_reaches_the_reader() {
        // BEL-terminated (clipboard) and ST-terminated (hyperlink) both.
        let clipboard = "\x1b]52;c;cHduZWQ=\x07real-output-line";
        assert_eq!(strip_ansi(clipboard), "real-output-line");

        let title = "x\x1b]0;pwned-title\x07y";
        assert_eq!(strip_ansi(title), "xy");

        let link = "see \x1b]8;;https://evil.test\x1b\\here\x1b]8;;\x1b\\ now";
        let out = strip_ansi(link);
        assert!(
            !out.contains("evil.test") && !out.contains('\x1b'),
            "the link target must go: {out:?}"
        );
        assert!(
            out.contains("here"),
            "and the text it wrapped must stay: {out:?}"
        );

        // Anti-vacuity, three ways. A stripper that ate everything, or that
        // only handled OSC, would pass the arms above and break the product.
        assert_eq!(strip_ansi("\x1b[31mRED\x1b[0m ok"), "RED ok");
        assert_eq!(
            strip_ansi("plain text, nothing to strip"),
            "plain text, nothing to strip"
        );
        assert_eq!(strip_ansi("50%\r100% done"), "100% done");
        // A lone `]` is ordinary text, not the start of anything.
        assert_eq!(strip_ansi("array[0] and json]"), "array[0] and json]");
    }
}

// The comment below describes `strip_ansi`. It was attached to the TEST
// module above, which it followed in the file: `rustdoc` published it
// for `osc_stripping` and the function had none.
/// Strip ANSI escape sequences and carriage-return progress redraws.
///
/// Handles both LF (`\n`) and CRLF (`\r\n`) line endings. On Windows where
/// most tool output is CRLF, a previous version reduced every line to
/// the empty string because `rsplit('\r').next()` on `"foo\r"` returns
/// the empty trailing piece, not `"foo"`. We now strip the trailing
/// CRLF carriage return before handling progress-bar redraws.
pub fn strip_ansi(s: &str) -> String {
    let s = ansi_re().replace_all(s, "");
    let s = osc_re().replace_all(&s, "");
    let s = esc_other_re().replace_all(&s, "").into_owned();
    s.split('\n')
        .map(|raw| {
            // 1) Drop the trailing carriage returns so a plain "line\r"
            //    stays "line", not "". ALL of them: Windows `nslookup` ends
            //    its header `\r\r\n`, and stripping one left `line\r`,
            //    whose last `\r` segment is empty — the line vanished
            //    (measured 2026-10-02, `Réponse ne faisant pas autorité`).
            //    A carriage return with nothing after it erases nothing.
            let line = raw.trim_end_matches('\r');
            // 2) Collapse in-line `\r`-redrawn progress text to the
            //    final state — `"50%\r100%"` becomes `"100%"`. If the
            //    line has no in-line `\r` the original survives intact.
            if line.contains('\r') {
                line.rsplit('\r').next().unwrap_or(line)
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod a_carriage_return_with_nothing_after_erases_nothing {
    use super::strip_ansi;

    /// Windows `nslookup`: `Réponse ne faisant pas autorité :\r\r\n`.
    #[test]
    fn a_double_cr_line_survives() {
        assert_eq!(
            strip_ansi("Réponse :\r\r\nServeur : one\r\n"),
            "Réponse :\nServeur : one\n"
        );
        // The redrawn progress bar keeps its last state.
        assert_eq!(strip_ansi("50%\r100%\r\n"), "100%\n");
    }
}
