//! `openssl` — OpenSSL CLI — drops the hex dumps, keeps the certificate.
//!
//! Archetype: strip

/// True for a line that is nothing but a hex byte dump — the modulus, the
/// signature value, a public key — as openssl prints them: indented, and
/// made of colon-separated pairs.
///
/// The rule used to be "starts with four spaces", which is what a hex dump
/// looks like and ALSO what every other line of `openssl x509 -text` looks
/// like: `Version`, `Serial Number`, `Issuer`, `Validity`, `Not After`,
/// `Subject`, the SANs and the key usage are all nested under
/// `Certificate:`. Measured 2026-09-02 on a real certificate bundle, that
/// left `Certificate:` and an elision count — 2 907 tokens became 15, and
/// every fact an agent opens a certificate to read was gone. Same shape as
/// the disassembly fold that read column alignment as nesting: indentation
/// is not a content type.
///
/// Four pairs minimum, so a short `aa:bb` inside prose is never taken for a
/// dump; a real openssl dump prints fifteen to eighteen per line.
fn is_hex_dump_line(line: &str) -> bool {
    if !line.starts_with("    ") {
        return false;
    }
    let t = line.trim();
    // ONE predicate, not two. An earlier pair — "every character is hex, a
    // colon or a space" AND "at least four two-character parts" — could not
    // be mutation-tested: each clause rejected everything the other would
    // have let through, so removing either left the output identical and
    // the guard could not tell a live clause from a dead one. Stated once,
    // it is exactly what openssl prints: colon-separated bytes.
    let mut parts = t.split(':').peekable();
    let mut bytes = 0usize;
    while let Some(part) = parts.next() {
        // A dump line may end on a colon, continuing on the next line.
        if part.is_empty() && parts.peek().is_none() {
            break;
        }
        if part.len() != 2 || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        bytes += 1;
    }
    bytes >= 4
}

/// A DUMP is a run, a VALUE is a line.
///
/// Shape alone cannot separate `0f:2e:5c:88:a1:04:9b:33` — an eight-byte
/// serial number, which an agent may well want — from one line of a
/// modulus, because they are the same thing typographically. What separates
/// them is how many of them there are: openssl prints a serial on one line
/// and a 2048-bit modulus on seventeen. So a hex line is dropped only when
/// it sits in a run of at least three, and a lone value survives.
const MIN_HEX_RUN: usize = 3;

fn strip_hex_dumps(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let hex: Vec<bool> = lines.iter().map(|l| is_hex_dump_line(l)).collect();
    let mut drop = vec![false; lines.len()];
    let mut i = 0usize;
    while i < lines.len() {
        if !hex[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && hex[i] {
            i += 1;
        }
        if i - start >= MIN_HEX_RUN {
            for d in drop.iter_mut().take(i).skip(start) {
                *d = true;
            }
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut elided = 0usize;
    for (line, dropped) in lines.iter().zip(drop) {
        if dropped {
            elided += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if elided > 0 {
        out.push_str(&format!("  [elided: {elided} indented hex line(s)]\n"));
    }
    out.trim_end().to_string()
}

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    Some(strip_hex_dumps(text))
}

pub fn compact(text: &str) -> String {
    strip_hex_dumps(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = "sample output line\nsecond line\n";
        let _ = try_compact(s);
        let _ = compact(s);
        let _ = compact_with_savings(s);
    }

    /// The hex goes, the certificate stays.
    ///
    /// The two halves OPPOSE each other on the only thing that separates
    /// them: both are indented under `Certificate:`, and only one is hex.
    /// Asserting the elision alone passed with the old prefix rule, which
    /// elided the whole document.
    #[test]
    fn the_hex_goes_and_the_certificate_stays() {
        let cert = "Certificate:\n    Data:\n        Version: 3 (0x2)\n        \
             Serial Number:\n            0f:2e:5c:88:a1:04:9b:33\n        \
             Issuer: C = US, O = Example Root CA, CN = Example Root R3\n        \
             Validity\n            Not Before: Jan  1 00:00:00 2026 GMT\n            \
             Not After : Sep  2 12:00:00 2027 GMT\n        \
             Subject: CN = billing.internal.example.com\n        \
             Subject Public Key Info:\n            Modulus:\n                \
             00:c2:9a:71:3f:e8:04:bb:5d:26:9f:31:7c:a0:48:\n                \
             12:7e:d5:b0:63:8a:cc:19:f4:20:6e:73:5a:91:dd:\n                \
             8b:47:02:e6:15:c9:38:70:ad:4f:63:2b:19:e0:74:\n        \
             X509v3 Subject Alternative Name:\n            \
             DNS:billing.internal.example.com\n";
        let out = compact(cert);
        for kept in [
            "Issuer: C = US",
            "Not After : Sep  2 12:00:00 2027 GMT",
            "Subject: CN = billing.internal.example.com",
            "DNS:billing.internal.example.com",
            "Version: 3 (0x2)",
        ] {
            assert!(out.contains(kept), "the certificate lost `{kept}`:\n{out}");
        }
        assert!(
            !out.contains("00:c2:9a:71"),
            "the modulus survived:\n{out}"
        );
        assert!(
            out.contains("3 indented hex line(s)"),
            "the three modulus lines were not elided, or were miscounted:\n{out}"
        );
        // A serial number is the same SHAPE as one line of modulus; only
        // the run length separates them, so it must survive on its own.
        assert!(
            out.contains("0f:2e:5c:88:a1:04:9b:33"),
            "the serial number was eaten:\n{out}"
        );

        // Indentation is part of the rule: openssl indents its dumps under
        // a label, and a top-level line is something else.
        let top = "aa:bb:cc:dd:ee\nab:cd:ef:01:23\n01:23:45:67:89\ntail\n";
        assert_eq!(
            compact(top),
            top.trim_end(),
            "an unindented run was eaten as a dump"
        );

        // And the bytes must be BYTES. Three indented lines of two-character
        // colon-separated parts that are not hex are not a dump, however
        // many of them there are.
        let nothex = "Label:\n    no:pe:no:pe:no\n    zz:yy:xx:ww:vv\n    qq:rr:ss:tt:uu\ntail\n";
        let out = compact(nothex);
        assert!(
            out.contains("no:pe:no:pe:no") && out.contains("qq:rr:ss:tt:uu"),
            "non-hex parts were eaten as a hex dump:\n{out}"
        );

        // IPv6 is hexadecimal and colon-separated and would be eaten by a
        // rule that only asked whether the characters are hex. Its groups
        // are one to four digits; a dump's are always exactly two. A SAN
        // list or an `s_client` connection log prints several in a row.
        let v6 = "IP Addresses:\n    2001:db8:85a3:0:0:8a2e:370:7334\n                      fe80:0:0:0:1c2e:99ff:fe0a:1\n    2606:4700:4700:0:0:0:0:1111\ntail\n";
        let out = compact(v6);
        assert!(
            out.contains("2001:db8:85a3") && out.contains("2606:4700:4700"),
            "an IPv6 run was eaten as a hex dump:\n{out}"
        );

        // And two bytes on a line is a VALUE, however many lines of it
        // there are — a dump prints fifteen to eighteen.
        let shorts = "Key Usage bytes:\n    ab:cd\n    ef:01\n    23:45\ntail\n";
        let out = compact(shorts);
        assert!(
            out.contains("ab:cd") && out.contains("23:45"),
            "two-byte values were eaten as a dump:\n{out}"
        );
    }
}
