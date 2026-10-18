//! **A wall of base64 is not a document.**
//!
//! MEASURED 2026-09-08 on the ledger: a `base64 payload.bin` of 48,833
//! bytes was delivered at 34,856 (−29 %), classified `auto:archive` — the archive
//! compactor SAMPLED lines that say nothing. Reproduced on the
//! `tokenade read` path, it is worse: the base64 of a 48,632-byte `tar`
//! comes back at **48,705**, bigger than the input, because nothing claims it
//! and the deliberate-read bar (25,000 tokens) serves it whole.
//!
//! None of these bytes is readable. What the agent may want from such a
//! block is to know WHAT it is, how much it weighs, and how to get the
//! bytes back — three facts, not forty-eight thousand characters.

/// Minimum length of a wrapped line. `base64` wraps at 76 (GNU default)
/// or 64 (MIME); below that we are no longer dealing with wrapping.
const MIN_WRAP: usize = 60;

/// Below this, it is not a block but a fragment — the body of a
/// certificate in a configuration file, a key in `known_hosts`.
/// Those belong to their document and are not folded here.
const MIN_LINES: usize = 12;

/// **Is this whole text a wrapped base64 block?**
///
/// "Whole", not "contains one": a certificate in the middle of a `.pem`, a
/// public key in `authorized_keys`, a base64 field in a JSON are
/// FRAGMENTS that belong to their document. Folding them would destroy the
/// document around them.
///
/// The alphabet alone is not enough — prose without spaces would pass it —
/// so three facts together: every line uses only the base64 alphabet
/// (a single space disqualifies it), the vast majority of lines have the
/// SAME length (it is wrapping, not prose), and the block carries
/// at least one character hexadecimal does not have (`+`, `/` or `=`) — without
/// which a list of upper-case sha256 sums would be taken for a blob.
pub fn looks_like_a_base64_blob(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < MIN_LINES {
        return false;
    }
    if !lines
        .iter()
        .all(|l| !l.is_empty() && l.bytes().all(is_base64_byte))
    {
        return false;
    }
    // The LAST line of a wrapping is shorter: we judge on the
    // others, and require their length to be common and wide.
    let body = &lines[..lines.len() - 1];
    let width = body[0].len();
    if width < MIN_WRAP {
        return false;
    }
    let same = body.iter().filter(|l| l.len() == width).count();
    if same * 10 < body.len() * 9 {
        return false;
    }
    if lines.last().is_some_and(|l| l.len() > width) {
        return false;
    }
    text.bytes().any(|b| matches!(b, b'+' | b'/' | b'='))
}

fn is_base64_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')
}

/// What we return for a base64 block: what it is, what it weighs, and what it
/// would weigh decoded. The bytes themselves go to the reserve through the
/// delivery path, like everything a fold removes.
pub fn compact(text: &str) -> String {
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    // 4 base64 characters carry 3 bytes; the final padding removes
    // as many as there are `=`.
    let chars: usize = text.bytes().filter(|b| is_base64_byte(*b)).count();
    let pad = text
        .trim_end()
        .bytes()
        .rev()
        .take_while(|b| *b == b'=')
        .count();
    let decoded = chars / 4 * 3 - pad.min(2);
    format!(
        "[tokenade: base64 block — {} bytes over {lines} lines, ~{} bytes once decoded. \
         None of it is readable text: ask for the bytes, not a transcription \
         (`tokenade expand-ref <hash> --raw`, hash in the banner below).]",
        text.len(),
        decoded
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(payload: &[u8], width: usize) -> String {
        let alphabet: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut enc = String::new();
        for c in payload.chunks(3) {
            let mut n = 0u32;
            for (i, b) in c.iter().enumerate() {
                n |= (*b as u32) << (16 - 8 * i);
            }
            for i in 0..4 {
                if i <= c.len() {
                    enc.push(alphabet[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    enc.push('=');
                }
            }
        }
        let mut out = String::new();
        for chunk in enc.as_bytes().chunks(width) {
            out.push_str(std::str::from_utf8(chunk).unwrap());
            out.push('\n');
        }
        out
    }

    #[test]
    fn a_wrapped_wall_of_base64_is_recognised() {
        let payload: Vec<u8> = (0..3_000u32).map(|i| (i % 251) as u8).collect();
        for width in [64, 76] {
            let b = blob(&payload, width);
            assert!(looks_like_a_base64_blob(&b), "largeur {width}");
        }
    }

    /// A FRAGMENT belongs to its document: folding it would destroy it.
    #[test]
    fn a_certificate_inside_a_document_is_left_alone() {
        let payload: Vec<u8> = (0..3_000u32).map(|i| (i % 251) as u8).collect();
        let body = blob(&payload, 64);
        let pem = format!("host example.com\n-----BEGIN CERTIFICATE-----\n{body}-----END CERTIFICATE-----\nport 443\n");
        assert!(!looks_like_a_base64_blob(&pem));
    }

    /// Nor what is not a wrapping: prose (the
    /// spaces), a list of hexadecimal sums (no `+`, no `/`, no `=`),
    /// a block too short, a last line longer than the others.
    #[test]
    fn what_is_not_a_wrapped_blob_is_refused() {
        let prose: String = (0..40)
            .map(|i| format!("ligne de prose numero {i}\n"))
            .collect();
        assert!(!looks_like_a_base64_blob(&prose));
        let hashes: String = (0..40).map(|i| format!("{:064x}\n", i * 7_919)).collect();
        assert!(
            !looks_like_a_base64_blob(&hashes),
            "sha256 sums are not a blob"
        );
        let payload: Vec<u8> = (0..3_000u32).map(|i| (i % 251) as u8).collect();
        let short: String = blob(&payload, 64)
            .lines()
            .take(6)
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(
            !looks_like_a_base64_blob(&short),
            "six lines are a fragment"
        );
        let ragged = format!("{}{}\n", blob(&payload, 64), "A".repeat(200));
        assert!(!looks_like_a_base64_blob(&ragged), "a longer last line");
        // Lines of different WIDTHS are not a wrapping:
        // a list of tokens or identifiers is made of them, and it carries
        // information we do not remove. (The `=` keep it in the
        // alphabet, so only the width tells it apart.)
        // All WIDE (≥ 60) — otherwise it is the minimum width that refuses and
        // the uniformity rule is never exercised: mutation showed it,
        // it survived a fixture that the neighbouring bound caught. And a
        // SHORT last line, so that the next check does not mask
        // the one we want to put to the test.
        let mut uneven: String = (0..39)
            .map(|i| format!("{}=\n", "Zm9vYmFy".repeat(8 + i % 9)))
            .collect();
        uneven.push_str("Zm9vYmFyZm9v\n");
        assert!(
            !looks_like_a_base64_blob(&uneven),
            "irregular widths are not a wrapping"
        );
        // Nor a NARROW wrapping: a column of short
        // identifiers is regular and remains content.
        let narrow: String = (0..40).map(|i| format!("id{:08}A+/=\n", i * 37)).collect();
        assert!(
            !looks_like_a_base64_blob(&narrow),
            "a narrow regular column remains content"
        );
    }

    /// The rendering states the three facts, and the decoded size is right.
    #[test]
    fn the_fold_states_what_it_is_and_how_big() {
        // The size of the REAL case: 36 KB of payload, 48,632 bytes of base64.
        // A small blob would give a flattering size ratio that says nothing
        // about the measured case.
        let payload: Vec<u8> = (0..36_000u32).map(|i| (i % 251) as u8).collect();
        let b = blob(&payload, 76);
        let out = compact(&b);
        assert!(out.contains("base64 block"), "{out}");
        assert!(out.contains("expand-ref"), "{out}");
        // 3,000 bytes ± padding: the announced size must be RIGHT,
        // otherwise it is one more claim not to believe.
        let decoded: usize = out
            .split("~")
            .nth(1)
            .and_then(|t| t.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .expect("the decoded size is announced");
        assert!(
            decoded.abs_diff(payload.len()) <= 2,
            "announced {decoded} for {} bytes",
            payload.len()
        );
        assert!(
            out.len() * 100 < b.len(),
            "the fold must save more than 99 %: {} of {}",
            out.len(),
            b.len()
        );
    }
}

#[cfg(test)]
mod corpus_guard {
    use super::*;

    /// **The false-positive risk is BOUNDED on real text, not on my
    /// intuition.**
    ///
    /// This detector removes all the content it claims: if it is wrong, the
    /// loss is total. The capture battery is the only corpus of real
    /// text this repo holds — command outputs, pages, logs,
    /// tables, in several languages — and none of it is a base64 block.
    #[test]
    fn no_real_capture_is_mistaken_for_a_base64_blob() {
        // The corpus lives in `tokenade-core`; `CARGO_MANIFEST_DIR` designates the
        // crate that COMPILES the file, and this module changed crates. The
        // floor further down (`checked > 200`) is what turns a wrong path
        // into a failure instead of a verdict returned on zero files.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tokenade-core/tests/fixtures");
        let mut stack = vec![root.clone()];
        let mut checked = 0usize;
        let mut wrong: Vec<String> = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for ent in entries.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&p) else {
                    continue;
                };
                checked += 1;
                if looks_like_a_base64_blob(&body) {
                    wrong.push(p.display().to_string());
                }
            }
        }
        assert!(
            checked > 200,
            "the guard no longer sees the battery ({checked} file(s)) — a guard \
             that reads nothing always passes"
        );
        assert!(
            wrong.is_empty(),
            "these REAL captures are taken for a base64 block, and all their \
             content would be removed:\n{}",
            wrong.join("\n")
        );
    }
}
