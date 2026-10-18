//! **Bytes, not text.**
//!
//! A child's output is not guaranteed to be UTF-8, and cutting it at a ceiling
//! almost guarantees the opposite: the last character is cut in two.
//!
//! Lossy decoding is acceptable; doing it SILENTLY is not. An
//! agent reading `U+FFFD` has no way to know whether the product corrupted the
//! output or whether it already was, so decoding announces what it
//! replaced and why.

// No `use super::*;`: decoding bytes needs nothing from launching
// a process — it is the only half of this module that does not talk to the OS.

/// How many byte sequences in `bytes` are not valid UTF-8.
///
/// Output that is not valid UTF-8 but is not recognisably BINARY either —
/// a `head -c N` that split a multibyte character, a latin-1 log, one stray
/// byte in an otherwise clean file — never reaches the binary short-circuit,
/// so it goes through the TEXT pipeline. `String::from_utf8_lossy` turns each
/// bad sequence into U+FFFD and says nothing about it, and the agent then
/// cannot tell OUR substitution from a replacement character the command
/// really printed. Counting them is what lets the caller say so.
///
/// The last sequence being *truncated* rather than invalid is the common
/// case and the interesting one: it is what a deliberate byte-count read
/// produces. `ends_truncated` reports it separately.
pub fn invalid_utf8_sequences(bytes: &[u8]) -> usize {
    let mut rest = bytes;
    let mut n = 0usize;
    loop {
        match std::str::from_utf8(rest) {
            Ok(_) => return n,
            Err(e) => {
                n += 1;
                let valid = e.valid_up_to();
                let skip = valid + e.error_len().unwrap_or(rest.len() - valid);
                if skip >= rest.len() {
                    return n;
                }
                rest = &rest[skip..];
            }
        }
    }
}

/// Whether the bytes end MID-CHARACTER — the shape a byte-count read
/// (`head -c N`, `dd bs=1 count=N`, `cut -b`) leaves behind. Distinguished
/// from a stray invalid byte because the remedy differs: one is the command
/// doing exactly what it was asked, the other is an encoding mismatch.
pub fn ends_mid_character(bytes: &[u8]) -> bool {
    matches!(std::str::from_utf8(bytes), Err(e) if e.error_len().is_none())
}

/// One line naming what the lossy decode changed, or `None` when nothing was
/// changed. Emitted on STDERR so stdout stays exactly what the fold produced.
pub fn lossy_decode_advisory(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let n = invalid_utf8_sequences(stdout) + invalid_utf8_sequences(stderr);
    if n == 0 {
        return None;
    }
    let why = if ends_mid_character(stdout) || ends_mid_character(stderr) {
        " (the output ends mid-character, which is what a byte-count read leaves behind)"
    } else {
        ""
    };
    Some(format!(
        "[tokenade] {n} byte sequence(s) in this output are not valid UTF-8 and are shown as U+FFFD{why}. \
         Those characters are tokenade's, not the command's.\n"
    ))
}

/// Decode captured child output to text, and say whether anything was lost.
///
/// `text_encoding::decode` exists for exactly these bytes — its module doc
/// names `from_utf8_lossy` "the culprit" and lists the encodings it destroys —
/// and had never been wired to COMMAND output. `read` and the stdin readers
/// decoded properly while every byte a wrapped child printed went through
/// `from_utf8_lossy`, so a French log, an Excel CSV `cat`ted, a PowerShell
/// redirect all reached the agent as mojibake, and the agent paid tokens for
/// the wreckage.
///
/// `false` in the second slot means bytes were REPLACED by U+FFFD, so the
/// caller can either hand back the original bytes (when it folded nothing) or
/// say what it substituted (when it did).
/// [`decode_child_output`], knowing which command printed `bytes`: a native
/// Windows console program writes in the OEM code page, and is read in it
/// (see [`crate::oem`]). Everything else is decoded as before.
pub fn decode_child_output_of(bytes: &[u8], shell_cmd: &str) -> (String, bool) {
    decode_child_output_with(bytes, shell_cmd, crate::oem::console_codepage())
}

/// The pure half of [`decode_child_output_of`], the code page passed in.
pub fn decode_child_output_with(bytes: &[u8], shell_cmd: &str, oem: Option<u32>) -> (String, bool) {
    if std::str::from_utf8(bytes).is_err() {
        let prog = shell_cmd.split_whitespace().next().unwrap_or("");
        if crate::oem::is_console_program(prog) {
            if let Some(text) = oem.and_then(|cp| crate::oem::decode(bytes, cp)) {
                return (text, true);
            }
        }
    }
    decode_child_output(bytes)
}

pub fn decode_child_output(bytes: &[u8]) -> (String, bool) {
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => return (s.to_string(), true),
        // A buffer whose ONLY fault is ending mid-character is UTF-8 that was
        // CUT, not a legacy encoding — `head -c N`, `dd bs=1`, `cut -b`.
        // chardetng always returns an answer, so handing it those bytes gets a
        // confident wrong one: measured 2026-08-26, `head -c 2` on `héllo`
        // (`68 c3`) came back as `hĂ`, which is worse than the U+FFFD it
        // replaced — a wrong character reads as the command's own, a
        // replacement character does not.
        Err(_) if ends_mid_character(bytes) && invalid_utf8_sequences(bytes) == 1 => {
            String::from_utf8_lossy(bytes).into_owned()
        }
        Err(_) => crate::text_encoding::decode(bytes)
            .map(|(t, _enc)| t)
            .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned()),
    };
    // Faithful is a property of the RESULT, not of which branch produced it.
    // `text_encoding::decode` answers `Some` for input that is UTF-8 with a
    // handful of corrupt sequences (`mostly_utf8`: up to one bad character per
    // thousand) — a `Some` that still carries U+FFFD. Reading the branch would
    // report those faithful and suppress BOTH the byte-exact path and the
    // advisory, on exactly the shape the advisory exists for: one stray byte
    // in a large otherwise-clean output.
    //
    // Counting rather than testing presence: output that legitimately contains
    // U+FFFD already (it is a printable character) must not be accused.
    let introduced = text.matches('\u{FFFD}').count()
        > bytes
            .windows(3)
            .filter(|w| *w == [0xEF, 0xBF, 0xBD])
            .count();
    (text, !introduced)
}
