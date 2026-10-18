//! Fold the base64 body of a PEM block, keep everything around it.
//!
//! A certificate, a key or a CSR printed in PEM form is a wall of base64 that
//! no model can read and that no line-based fold touches: every line is a
//! different 64-character string, so dedup finds nothing, and a chain of them
//! still fits under the generic line cap. MEASURED 2026-09-06 on a live
//! `openssl s_client -connect tokenade.net:443 -showcerts`: **6 572 bytes, of
//! which 4 416 (67%) are certificate base64**, and both the generic route and
//! openssl's own compactor returned it at **0.0% saved**.
//!
//! Nothing in that body is knowable by reading it. The fields a reader wants —
//! subject, issuer, validity, the verify code — are printed in the clear by the
//! same command, right beside the block. So the body is replaced by a line that
//! says what stood there, and the verbatim bytes stay recoverable through the
//! usual stash.
//!
//! The delimiters are explicit and standard (RFC 7468), so this is a parse and
//! not a sniff: a block folds only when it OPENS and CLOSES with the same
//! label and its body is base64 all the way through. An unterminated block is
//! left alone — what cannot be bounded is not eaten.
//!
//! KNOWN LIMIT, measured rather than assumed: this runs as a post-pass, so a
//! block the routed answer has already cut in half no longer closes and is
//! left alone. That needs a SINGLE PEM above the output cap — about 200 KB,
//! some fifty times a real certificate — and at that size the cap has already
//! saved 98.5%. Realistic shapes are covered: a live `s_client -showcerts`
//! goes 0.0% → 66.5%, a 4.8 KB certificate file → 86%, a 40 KB CA bundle of
//! 106 certificates → 70%. A second copy of this pass earlier in the pipeline
//! would close that gap and cost more than it buys: a rule spelled twice is a
//! rule neither copy can be tested for.

/// Below this many body lines a block is left alone: the marker is about as
/// long as the body, so folding would buy nothing and lose the bytes.
const MIN_BODY_LINES: usize = 4;

/// `-----BEGIN X-----` / `-----END X-----`, returning the label.
fn delimiter(line: &str, keyword: &str) -> Option<String> {
    let t = line.trim();
    let rest = t.strip_prefix("-----")?.strip_suffix("-----")?;
    let label = rest.strip_prefix(keyword)?.strip_prefix(' ')?;
    if label.is_empty() {
        return None;
    }
    Some(label.to_string())
}

/// An OpenPGP armor header — `Version: GnuPG v2.4.5`, `Comment: …`, `Hash: …`.
///
/// RFC 4880 §6.2 puts these immediately after the BEGIN line, ended by one
/// blank line, before the base64. They are part of the declared format, not
/// prose: without recognising them a `.asc` public key never closed and came
/// back whole — MEASURED 2026-09-06 at 0.0% saved on a 3 366-byte armored key.
/// They are SHORT and they say what wrote the block, so they are kept.
fn is_armor_header(line: &str) -> bool {
    let t = line.trim();
    let Some((key, _)) = t.split_once(": ") else {
        return false;
    };
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A PEM body line: base64 alphabet and nothing else. Empty lines are allowed
/// inside a block (some emitters pad), but a line of prose ends the attempt.
fn is_body_line(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// Replace each PEM block's body with a marker naming what it replaced.
///
/// Returns the rewritten text, how many blocks were folded, and how many body
/// lines went away. Both counts are zero exactly when the text comes back
/// unchanged, which is what lets a caller stay silent on output that has no
/// PEM in it.
pub fn fold_pem_blocks(text: &str) -> (String, usize, usize) {
    if !text.contains("-----BEGIN ") {
        return (text.to_string(), 0, 0);
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut blocks = 0usize;
    let mut dropped = 0usize;
    let mut i = 0usize;
    while i < lines.len() {
        let Some(label) = delimiter(lines[i], "BEGIN") else {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        };
        // Armor headers first, if any: they sit between BEGIN and the base64
        // and are kept verbatim, so the body starts after them.
        let mut body_from = i + 1;
        while body_from < lines.len() && is_armor_header(lines[body_from]) {
            body_from += 1;
        }
        // Scan for this label's END, over body lines only.
        let mut j = body_from;
        while j < lines.len() && is_body_line(lines[j]) {
            j += 1;
        }
        let closes = j < lines.len() && delimiter(lines[j], "END").as_deref() == Some(&label);
        let body = j - body_from;
        if !closes || body < MIN_BODY_LINES {
            // Unterminated, mislabelled, or too short to be worth it: verbatim.
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        let bytes: usize = lines[body_from..j].iter().map(|l| l.len() + 1).sum();
        let indent: String = lines[i].chars().take_while(|c| c.is_whitespace()).collect();
        out.push(lines[i].to_string());
        for header in &lines[i + 1..body_from] {
            out.push(header.to_string());
        }
        out.push(format!(
            "{indent}… {body} line(s) of base64 elided by tokenade ({bytes} bytes) …"
        ));
        out.push(lines[j].to_string());
        blocks += 1;
        dropped += body;
        i = j + 1;
    }
    if blocks == 0 {
        return (text.to_string(), 0, 0);
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    (s, blocks, dropped)
}

// The `mod wiring` that used to live here — "the fold must be REACHED, and by every
// command route" — moved to
// `tests/misc/a_format_is_reached_by_its_router.rs`: it queries `filter`,
// which stays in `tokenade-core` while this module moved to `tk-formats`.

#[cfg(test)]
mod tests {
    use super::*;

    fn pem(label: &str, body_lines: usize) -> String {
        let body: Vec<String> = (0..body_lines)
            .map(|n| format!("{:A<64}", format!("Ln{n}")))
            .collect();
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            body.join("\n")
        )
    }

    #[test]
    fn a_certificate_body_is_replaced_and_the_delimiters_stay() {
        let text = format!("subject=CN=x\n{}verify return:1\n", pem("CERTIFICATE", 24));
        let (out, blocks, dropped) = fold_pem_blocks(&text);
        assert_eq!((blocks, dropped), (1, 24));
        assert!(out.contains("-----BEGIN CERTIFICATE-----"));
        assert!(out.contains("-----END CERTIFICATE-----"));
        assert!(out.contains("24 line(s) of base64 elided by tokenade"));
        // The signal on both sides survives, and the body does not.
        assert!(out.contains("subject=CN=x") && out.contains("verify return:1"));
        assert!(!out.contains("Ln7AAAA"), "a body line survived: {out}");
        assert!(
            out.len() * 4 < text.len(),
            "no real saving: {} vs {}",
            out.len(),
            text.len()
        );
    }

    /// OpenPGP armor puts `Key: value` headers between BEGIN and the base64
    /// (RFC 4880 §6.2). Without them recognised the block never closed, and a
    /// `.asc` public key came back whole — measured at 0.0% saved on 3 366
    /// bytes, 95.0% now. They are kept: they are short and they say what wrote
    /// the block.
    #[test]
    fn an_armored_block_folds_and_keeps_its_headers() {
        let body: Vec<String> = (0..52)
            .map(|n| format!("{:A<64}", format!("b{n}")))
            .collect();
        let text = format!(
            "-----BEGIN PGP PUBLIC KEY BLOCK-----\nVersion: GnuPG v2.4.5\nComment: a key\n\n{}\n=aB3d\n-----END PGP PUBLIC KEY BLOCK-----\n",
            body.join("\n")
        );
        let (out, blocks, _) = fold_pem_blocks(&text);
        assert_eq!(blocks, 1, "the armored block did not fold: {out}");
        assert!(out.contains("Version: GnuPG v2.4.5"), "header lost: {out}");
        assert!(out.contains("Comment: a key"), "header lost: {out}");
        // The COUNT must be the body, not the body plus the headers: 52 base64
        // lines, one blank, one CRC line — the two headers are kept and are not
        // part of what was elided. A count taken from the BEGIN line instead of
        // from the first body line reads 56 here, and nothing else notices.
        assert!(
            out.contains("… 54 line(s) of base64 elided by tokenade"),
            "the elided count is not the body: {out}"
        );
        assert!(out.contains("-----END PGP PUBLIC KEY BLOCK-----"));
        assert!(!out.contains("b7AAAA"), "a body line survived: {out}");
        assert!(
            out.len() * 8 < text.len(),
            "{} of {}",
            out.len(),
            text.len()
        );
    }

    /// A header-shaped line is only a header where the format puts one. After
    /// the base64 has begun, `Note: something` is prose and ends the block.
    #[test]
    fn a_header_after_the_body_has_started_is_not_a_header() {
        let text = "-----BEGIN PGP MESSAGE-----\nAAAA\nBBBB\nNote: injected\nCCCC\nDDDD\n-----END PGP MESSAGE-----\n";
        assert_eq!(fold_pem_blocks(text), (text.to_string(), 0, 0));
    }

    #[test]
    fn a_chain_folds_every_block() {
        let text = format!(
            "{}{}{}",
            pem("CERTIFICATE", 20),
            pem("CERTIFICATE", 30),
            pem("CERTIFICATE", 25)
        );
        let (out, blocks, dropped) = fold_pem_blocks(&text);
        assert_eq!((blocks, dropped), (3, 75));
        assert_eq!(out.matches("-----BEGIN CERTIFICATE-----").count(), 3);
        assert_eq!(out.matches("elided by tokenade").count(), 3);
    }

    /// The anti-vacuity arm: "fold everything with BEGIN in it" must not pass.
    #[test]
    fn what_must_not_be_folded_is_not() {
        // Unterminated: nothing bounds it, so nothing is eaten.
        let torn = "-----BEGIN CERTIFICATE-----\nAAAA\nBBBB\nCCCC\nDDDD\nEEEE\n";
        assert_eq!(fold_pem_blocks(torn), (torn.to_string(), 0, 0));

        // Mismatched label.
        let mixed =
            "-----BEGIN CERTIFICATE-----\nAAAA\nBBBB\nCCCC\nDDDD\n-----END PRIVATE KEY-----\n";
        assert_eq!(fold_pem_blocks(mixed), (mixed.to_string(), 0, 0));

        // Too short to be worth it.
        let tiny = "-----BEGIN CERTIFICATE-----\nAAAA\nBBBB\n-----END CERTIFICATE-----\n";
        assert_eq!(fold_pem_blocks(tiny), (tiny.to_string(), 0, 0));

        // Prose inside: not a body, so the block never closes over it.
        let prose = "-----BEGIN CERTIFICATE-----\nAAAA\nthis is not base64!\nBBBB\nCCCC\nDDDD\n-----END CERTIFICATE-----\n";
        assert_eq!(fold_pem_blocks(prose), (prose.to_string(), 0, 0));

        // No PEM at all comes back identical, and says so with zeroes.
        let plain = "subject=CN=x\nissuer=CN=y\nverify return:1\n";
        assert_eq!(fold_pem_blocks(plain), (plain.to_string(), 0, 0));
    }

    #[test]
    fn an_indented_block_keeps_its_indentation() {
        let inner = pem("PRIVATE KEY", 12);
        let text: String = inner.lines().map(|l| format!("    {l}\n")).collect();
        let (out, blocks, _) = fold_pem_blocks(&text);
        assert_eq!(blocks, 1);
        assert!(
            out.contains("    … 12 line(s) of base64 elided by tokenade"),
            "indentation lost: {out}"
        );
    }

    #[test]
    fn a_trailing_newline_is_preserved_either_way() {
        let with = pem("CERTIFICATE", 10);
        assert!(fold_pem_blocks(&with).0.ends_with('\n'));
        let without = with.trim_end_matches('\n').to_string();
        assert!(!fold_pem_blocks(&without).0.ends_with('\n'));
    }
}
