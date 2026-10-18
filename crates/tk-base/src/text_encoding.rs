//! Legacy text-encoding decode for documents.
//!
//! Not every text file an agent opens is UTF-8, and the ones that aren't are
//! precisely the ones humans care about most: a French CSV exported from Excel
//! is Windows-1252, a Japanese one is Shift-JIS, a Chinese one GBK, and UTF-16
//! comes straight out of PowerShell redirects and Windows tooling.
//!
//! Measured before this module existed, through the live `tokenade web` path:
//!
//! | input          | agent saw                       |
//! |----------------|---------------------------------|
//! | Shift-JIS CSV  | `�n��,��荞��,�G���[`           |
//! | GBK CSV        | `����,��ȡ,����`                 |
//! | Latin-1 CSV    | `R�gion,D�bit` (accents gone)   |
//! | UTF-16 CSV     | classed BINARY, unreadable      |
//!
//! `String::from_utf8_lossy` is the culprit: it maps every undecodable byte to
//! U+FFFD, so the content is destroyed *and* the agent still pays tokens for the
//! wreckage — then re-reads the file, paying twice for nothing.
//!
//! Pure Rust (`encoding_rs` + `chardetng`, both Mozilla's), so the multi-OS
//! constraint holds: no iconv, no system locale, identical result on every box.

/// Decode `bytes` to text, honouring a BOM, then UTF-8, then a detected legacy
/// encoding. Returns the text and the encoding name that produced it.
///
/// `None` means "this is not text" — the caller should treat it as binary
/// rather than print mojibake.
pub fn decode(bytes: &[u8]) -> Option<(String, &'static str)> {
    if bytes.is_empty() {
        return None;
    }
    // 1. A BOM is a declaration, not a guess — it wins outright. This is also
    //    what rescues UTF-16: its NUL bytes make every binary sniffer call it
    //    binary, so without the BOM check it never reaches a text path at all.
    if let Some((enc, without_bom)) = bom_encoding(bytes) {
        let (text, _, malformed) = enc.decode(without_bom);
        if !malformed {
            return Some((text.into_owned(), enc.name()));
        }
    }
    // 2. Valid UTF-8 → done, no detection cost on the common path. Still
    //    screened: control bytes are perfectly valid UTF-8, so binary that
    //    happens to decode would sail through on validity alone.
    if let Ok(s) = std::str::from_utf8(bytes) {
        return looks_like_text(s).then(|| (s.to_string(), "UTF-8"));
    }
    // 2b. Bytes that are UTF-8 apart from a handful of corrupt sequences are
    //     UTF-8 WITH CORRUPTION, not a legacy encoding. Without this rule a
    //     single stray byte demoted the whole document to step 3, and
    //     chardetng — asked to name the encoding of what really is UTF-8 —
    //     answered windows-1252, so every multi-byte sequence came back as
    //     mojibake. MEASURED on o2switch.fr: one invalid byte at offset
    //     339 907 of a 340 KB page turned "Hébergeur de Qualité" into
    //     "HÃ©bergeur de QualitÃ©" across the entire render.
    //
    //     The ratio is what separates the two cases: a genuinely Latin-1 (or
    //     Shift-JIS, or GBK) document read as UTF-8 fails on EVERY accented
    //     byte — percent-level, not per-mille — so it still falls through.
    if let Some(s) = mostly_utf8(bytes) {
        return looks_like_text(&s).then_some((s, "UTF-8"));
    }
    // 3. Legacy encoding. chardetng always returns *an* answer, so verify it:
    //    a wrong guess (or actual binary) shows up as replacement characters.
    let mut det = chardetng::EncodingDetector::new();
    det.feed(bytes, true);
    let enc = det.guess(None, true);
    let (text, _, malformed) = enc.decode(bytes);
    if malformed || !looks_like_text(&text) {
        return None;
    }
    Some((text.into_owned(), enc.name()))
}

/// The encoding a BOM declares, plus the bytes after it.
fn bom_encoding(bytes: &[u8]) -> Option<(&'static encoding_rs::Encoding, &[u8])> {
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => Some((encoding_rs::UTF_8, rest)),
        [0xFF, 0xFE, rest @ ..] => Some((encoding_rs::UTF_16LE, rest)),
        [0xFE, 0xFF, rest @ ..] => Some((encoding_rs::UTF_16BE, rest)),
        _ => None,
    }
}

/// One invalid byte per thousand characters is corruption in a UTF-8 stream;
/// one per hundred is the wrong encoding. Returns the lossy UTF-8 text when
/// the input sits on the corruption side of that line.
///
/// The threshold is per-MILLE deliberately: the tightest real legacy case is a
/// Western European document, where roughly one character in twenty carries an
/// accent — twenty times over the line even before Shift-JIS or GBK, whose
/// every text byte is undecodable.
fn mostly_utf8(bytes: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(bytes);
    let mut chars = 0usize;
    let mut bad = 0usize;
    for c in s.chars() {
        chars += 1;
        if c == '\u{FFFD}' {
            bad += 1;
        }
    }
    // A short input gives the ratio nothing to work with, so it is left to the
    // detector, which is better at short samples than an arbitrary floor is.
    (chars >= 1_000 && bad * 1_000 < chars).then(|| s.into_owned())
}

/// Reject a decode that produced rubbish. Any single-byte encoding maps every
/// byte to *some* character, so a JPEG "decodes" to Latin-1 garbage without a
/// single error — the errors-based check alone would happily print it.
fn looks_like_text(s: &str) -> bool {
    let mut controls = 0usize;
    let mut total = 0usize;
    for c in s.chars().take(4096) {
        total += 1;
        // Control characters outside the ones text legitimately uses.
        if (c.is_control() && !matches!(c, '\n' | '\r' | '\t')) || c == '\u{FFFD}' {
            controls += 1;
        }
    }
    if total == 0 {
        return false;
    }
    // Real text is overwhelmingly printable; binary is dense in control bytes.
    (controls * 100 / total) < 2
}

/// `Content-Type` reduced to a bare lowercase type, no parameters.
pub fn normalized_content_type(ct: Option<&str>) -> Option<String> {
    ct.map(|c| {
        c.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    })
    .filter(|c| !c.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_without_guessing() {
        let (t, enc) = decode("héllo → ok".as_bytes()).unwrap();
        assert_eq!(t, "héllo → ok");
        assert_eq!(enc, "UTF-8");
    }

    /// One corrupt byte must not demote a UTF-8 document to a legacy guess.
    /// Regression: o2switch.fr shipped 340 KB of valid UTF-8 with a single
    /// invalid byte in it, and the whole page rendered as windows-1252.
    #[test]
    fn one_bad_byte_does_not_turn_a_utf8_page_into_mojibake() {
        let mut page = "<p>Hébergeur Web de Qualité Industrielle</p>\n"
            .repeat(60)
            .into_bytes();
        assert!(page.len() > 1_000);
        page.insert(page.len() / 2, 0xED); // lone continuation-less byte
        let (t, enc) = decode(&page).unwrap();
        assert_eq!(enc, "UTF-8");
        assert!(
            t.contains("Hébergeur Web de Qualité"),
            "mojibake: {}",
            &t[..80]
        );
    }

    /// The escape hatch above must not swallow real legacy text: a Latin-1
    /// document fails on every accent, which is percent-level, not per-mille.
    #[test]
    fn a_long_latin1_document_still_reaches_the_detector() {
        let latin1 = encoding_rs::WINDOWS_1252
            .encode(&"Région Débit élevé Qualité\n".repeat(80))
            .0
            .into_owned();
        assert!(latin1.len() > 1_000);
        let (t, enc) = decode(&latin1).unwrap();
        assert_ne!(enc, "UTF-8");
        assert!(
            t.contains("Région") && t.contains("élevé"),
            "latin-1 lost: {t}"
        );
    }

    /// The four encodings measured as broken through the live path.
    #[test]
    fn decodes_legacy_encodings_that_used_to_become_mojibake() {
        // Latin-1: French accents were destroyed (`R�gion`, `�lev�`).
        let latin1 = encoding_rs::WINDOWS_1252
            .encode("Région,Débit\nNEEDLE-L1,élevé")
            .0
            .into_owned();
        let (t, _) = decode(&latin1).unwrap();
        assert!(
            t.contains("Région") && t.contains("élevé"),
            "latin-1 lost: {t}"
        );

        // Shift-JIS.
        let sjis = encoding_rs::SHIFT_JIS
            .encode("地域,取り込み\nNEEDLE-SJIS,ストリーム")
            .0
            .into_owned();
        let (t, _) = decode(&sjis).unwrap();
        assert!(
            t.contains("地域") && t.contains("ストリーム"),
            "shift-jis lost: {t}"
        );

        // GBK.
        let gbk = encoding_rs::GBK
            .encode("区域,摄取\nNEEDLE-GBK,流")
            .0
            .into_owned();
        let (t, _) = decode(&gbk).unwrap();
        assert!(t.contains("区域"), "gbk lost: {t}");
    }

    /// UTF-16 is full of NUL bytes, so every binary sniffer calls it binary and
    /// the agent got a stash reference instead of a readable file. The BOM is
    /// authoritative and must win before any sniffing.
    #[test]
    fn decodes_utf16_via_bom() {
        let mut le = vec![0xFF, 0xFE];
        for u in "Region,Ingest\nNEEDLE-U16,stream".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        let (t, enc) = decode(&le).unwrap();
        assert!(t.contains("NEEDLE-U16"), "utf-16le lost: {t}");
        assert_eq!(enc, "UTF-16LE");

        let mut be = vec![0xFE, 0xFF];
        for u in "NEEDLE-BE".encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert!(decode(&be).unwrap().0.contains("NEEDLE-BE"));

        // UTF-8 BOM must not survive into the text (it would poison a first-line
        // parse — a CSV header becomes "\u{FEFF}Region").
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice("Region".as_bytes());
        assert_eq!(decode(&b).unwrap().0, "Region");
    }

    /// A single-byte encoding maps every byte to something, so binary decodes
    /// "successfully" into garbage. Printing that is the bug we are fixing, not
    /// a lesser version of it.
    #[test]
    fn refuses_binary_rather_than_returning_garbage() {
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x01\x00\x00\x00\x01\x00\x08\x06\x00\x00\x00";
        assert!(decode(png).is_none(), "binary decoded as text");
        let mut noise = Vec::new();
        for i in 0..512u32 {
            noise.push((i % 7) as u8); // dense control bytes
        }
        assert!(
            decode(&noise).is_none(),
            "control-byte noise decoded as text"
        );
        assert!(decode(b"").is_none());
    }
}

/// Finish a byte-accumulating scanner into a `String`.
///
/// Compactors whose delimiters are all ASCII are correctly written as byte
/// scanners: UTF-8 is self-synchronising, so no ASCII byte ever appears inside a
/// multibyte sequence and the scan cannot desynchronise. The mistake is to
/// *emit* through `b as char`, which reinterprets every byte above 0x7F as its
/// Latin-1 codepoint and hands the caller mojibake. Accumulate `Vec<u8>` and
/// finish here instead.
///
/// Lossy on purpose: these scanners can be handed a file that is not valid UTF-8
/// to begin with, and replacing the bad bytes is strictly better than refusing
/// to compact or panicking.
pub fn utf8_owned(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Percent-decoding
// ─────────────────────────────────────────────────────────────────────
//
// Three copies lived in the tree: `serp::percent_decode`,
// `agent_copilot::percent_decode` and `agent_grok::url_decode`. They
// were not identical, and that is precisely the problem: one
// operation written three times drifts into three behaviours, none of which is
// declared. The two real differences became ARGUMENTS here, so that a
// caller chooses by saying so rather than by copying.

/// The bytes of `s`, its `%XX` sequences replaced by the byte they
/// spell. `plus_is_space` treats `+` as a space, as
/// `application/x-www-form-urlencoded` does — a QUERY string — and as a path
/// does NOT.
///
/// A truncated or non-hexadecimal `%` sequence is left as is:
/// a literal `%` in a path is common and eating it would be a loss.
fn percent_decode_bytes(s: &str, plus_is_space: bool) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hi = (b[i + 1] as char).to_digit(16);
            let lo = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        if plus_is_space && b[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    out
}

/// Percent-decoding of a PATH or an identifier: `+` stays a `+`, and
/// bytes that do not form UTF-8 go through lossy
/// replacement. This is the shape agent transcript readers expect,
/// where the decoded string serves as a display key.
pub fn percent_decode(s: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(s, false)).into_owned()
}

/// Percent-decoding of a QUERY string: `+` means a space, and a
/// result that is not UTF-8 returns the ORIGINAL string rather than
/// U+FFFD. For an address, keeping the encoded form is an answer, a
/// replacement character is not.
pub fn form_decode(s: &str) -> String {
    String::from_utf8(percent_decode_bytes(s, true)).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod percent_decoding {
    use super::*;

    /// The two differences between the three copies, held in both directions.
    #[test]
    fn a_plus_is_a_space_only_in_a_query() {
        assert_eq!(form_decode("a+b"), "a b");
        assert_eq!(percent_decode("a+b"), "a+b");
        assert_eq!(form_decode("%2Fhome%2Fa+b"), "/home/a b");
    }

    #[test]
    fn a_result_that_is_not_utf8_keeps_the_encoded_form_for_a_query() {
        // `%FF` alone is not valid UTF-8.
        assert_eq!(form_decode("x%FFy"), "x%FFy");
        assert_eq!(percent_decode("x%FFy"), "x\u{FFFD}y");
    }

    /// A `%` that does not open a sequence stays a `%`: eating it would be a
    /// silent loss on a path carrying a literal one.
    #[test]
    fn a_truncated_or_non_hex_escape_is_left_alone() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zz"), "a%zz");
        assert_eq!(percent_decode("%41"), "A");
    }

    #[test]
    fn normalizes_content_type() {
        assert_eq!(
            normalized_content_type(Some("Text/HTML; charset=UTF-8")).as_deref(),
            Some("text/html")
        );
        assert!(normalized_content_type(Some("  ")).is_none());
        assert!(normalized_content_type(None).is_none());
    }
}
