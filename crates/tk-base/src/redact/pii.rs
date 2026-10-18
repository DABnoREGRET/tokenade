//! **Personal data, and why a number is never enough.**
//!
//! An IBAN, an account number, an NPI, an NHS number: these are NUMBERS, and the
//! world is full of them. Every rule here requires TWO things — the format's
//! checksum, and a nearby word saying what it is. One
//! without the other redacts build identifiers by the thousand.

use super::*;

// ---------------------------------------------------------------------------
// F — Kubernetes Secret manifests. base64 (or stringData plaintext) values in
// a `data:` map only make sense to redact when we KNOW the doc is a Secret, so
// we gate hard on a literal `kind: Secret` appearing anywhere in the text.
// Conservative: without that gate this returns the input byte-for-byte.
// ---------------------------------------------------------------------------
pub(super) fn redact_k8s_secret(s: &str) -> String {
    if !(s.contains("kind: Secret") || s.contains("kind:Secret")) {
        return s.to_string();
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // A YAML `  key: <base64>` value line. base64 body ≥16 chars keeps
        // short scalars (v1, Secret, Opaque, true) untouched.
        Regex::new(r"(?m)^(\s*[\w.\-]+:\s*)[A-Za-z0-9+/]{16,}={0,2}\s*$").unwrap()
    });
    re.replace_all(s, "${1}<redacted>").into_owned()
}

// ---------------------------------------------------------------------------
// I — email + phone + IP + MAC. OPT-IN ONLY (`TOKENADE_REDACT_PII`, default
// OFF; legacy alias `TOKENADE_REDACT_EMAIL_PHONE`). These four appear
// constantly in ordinary command output (`git log`/`Author:`, `netstat`,
// `docker inspect`, `ifconfig`, application logs), so masking them by default
// costs utility for a redaction the user did not ask for. IBAN & SSN used to
// live here but are now DEFAULT-ON in `redact_pii_structured` (structurally
// self-anchoring, so they don't degrade ordinary text). Conservative: emails
// keep `@domain`; phone requires an explicit `+` E.164 form preceded by a
// separator (dodges diff `+`-markers).
// ---------------------------------------------------------------------------
pub(super) fn redact_optin_pii(s: &str) -> String {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        vec![
            // email — mask local part, keep `@domain`. `<redacted>@domain` is
            // not re-matched (the `>` before `@` is not a local-part char).
            (
                Regex::new(r"[A-Za-z0-9._%+\-]+(@[A-Za-z0-9.\-]+\.[A-Za-z]{2,})").unwrap(),
                "<redacted>${1}",
            ),
            // phone — E.164, only when preceded by a separator so a diff/patch
            // line marker (`+123…` at column 0) never fires.
            (
                Regex::new(r"([ (\t:])\+\d{7,15}\b").unwrap(),
                "${1}<redacted>",
            ),
            // conservative NANP-style grouped forms: `(415) 555-2671`,
            // `415-555-2671`, `415.555.2671`. Opt-in only, so the residual FP
            // on grouped 10-digit runs is acceptable.
            (
                Regex::new(r"\(\d{3}\)\s?\d{3}[-.\s]\d{4}\b").unwrap(),
                "<redacted>",
            ),
            (
                Regex::new(r"\b\d{3}[-.]\d{3}[-.]\d{4}\b").unwrap(),
                "<redacted>",
            ),
        ]
    });
    let mut out = s.to_string();
    for (re, rep) in rules {
        out = re.replace_all(&out, *rep).into_owned();
    }
    // IP + MAC — opt-in for the same reason (ubiquitous in netstat/ps/docker/
    // ifconfig). Structural rules, validated so a 4-part version string like
    // `1.2.3.4` (octet ≤255 but not a real address in context) still matches
    // ONLY here, never by default.
    static IPV6: OnceLock<Regex> = OnceLock::new();
    static IPV4: OnceLock<Regex> = OnceLock::new();
    static MAC: OnceLock<Regex> = OnceLock::new();
    let ipv6 = IPV6.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}\b|(?i)(?:[0-9a-f]{1,4}:){1,7}:(?:[0-9a-f]{1,4}:?){0,6}[0-9a-f]{0,4}",
        )
        .unwrap()
    });
    out = mask_re(&out, ipv6, |_, m| {
        let t = m.as_str();
        t.matches(':').count() >= 2 && (t.contains("::") || t.matches(':').count() == 7)
    });
    let ipv4 = IPV4.get_or_init(|| Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap());
    out = mask_re(&out, ipv4, |_, m| ipv4_octets_ok(m.as_str()));
    let mac = MAC.get_or_init(|| Regex::new(r"\b(?:[0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}\b").unwrap());
    out = mask_re(&out, mac, |_, _| true);
    out
}

/// Two-stage Presidio-style decision, fully deterministic:
///   stage 1 (validate): a passing checksum is a decisive, stand-alone signal;
///   stage 2 (context):  when the pattern alone can't disambiguate, a nearby
///                       keyword supplies the missing confidence.
/// A *failed* checksum is a hard reject handled at the call-site (which simply
/// doesn't pass `checksum_ok = true`), so this returns false and the candidate
/// is kept verbatim.
pub(super) fn validated(checksum_ok: bool, has_context: bool) -> bool {
    checksum_ok || has_context
}

// --- checksums -------------------------------------------------------------

/// IBAN mod-97 (ISO 13616): move the first 4 chars to the end, map letters to
/// two-digit numbers (A=10…Z=35), fold the huge integer mod 97 → must be 1.
pub(super) fn iban_mod97(iban: &str) -> bool {
    let up = iban.to_ascii_uppercase();
    let b = up.as_bytes();
    if b.len() < 5 {
        return false;
    }
    let mut rem: u32 = 0;
    // rearranged = iban[4..] ++ iban[0..4]
    let rearranged = b[4..].iter().chain(b[0..4].iter());
    for &c in rearranged {
        match c {
            b'0'..=b'9' => rem = (rem * 10 + (c - b'0') as u32) % 97,
            b'A'..=b'Z' => {
                let v = (c - b'A' + 10) as u32; // two digits
                rem = (rem * 100 + v) % 97;
            }
            _ => return false,
        }
    }
    rem == 1
}

/// The ISO 13616 registry: an IBAN's country code fixes its EXACT length.
/// Mod-97 alone passes one random string in 97, and a 26-character run of
/// capitals and digits opening on two letters and two digits is everyday in
/// tool output — measured 2026-10-02 on a real `mount`: one Docker overlay
/// short id (`l/AI…`) of six came back `<redacted>`, a 404-character mount
/// option list with it. `AI` is no IBAN country, and no IBAN country is 26
/// long but Iceland and Turkey.
pub(super) fn iban_country_length_ok(iban: &str) -> bool {
    let up = iban.to_ascii_uppercase();
    IBAN_LENGTHS
        .iter()
        .any(|&(cc, len)| up.len() == len && up.starts_with(cc))
}

/// Country code → IBAN length, from the ISO 13616 registry.
pub(super) const IBAN_LENGTHS: &[(&str, usize)] = &[
    ("AD", 24),
    ("AE", 23),
    ("AL", 28),
    ("AT", 20),
    ("AZ", 28),
    ("BA", 20),
    ("BE", 16),
    ("BG", 22),
    ("BH", 22),
    ("BI", 27),
    ("BR", 29),
    ("BY", 28),
    ("CH", 21),
    ("CR", 22),
    ("CY", 28),
    ("CZ", 24),
    ("DE", 22),
    ("DJ", 27),
    ("DK", 18),
    ("DO", 28),
    ("EE", 20),
    ("EG", 29),
    ("ES", 24),
    ("FI", 18),
    ("FK", 18),
    ("FO", 18),
    ("FR", 27),
    ("GB", 22),
    ("GE", 22),
    ("GI", 23),
    ("GL", 18),
    ("GR", 27),
    ("GT", 28),
    ("HN", 28),
    ("HR", 21),
    ("HU", 28),
    ("IE", 22),
    ("IL", 23),
    ("IQ", 23),
    ("IS", 26),
    ("IT", 27),
    ("JO", 30),
    ("KW", 30),
    ("KZ", 20),
    ("LB", 28),
    ("LC", 32),
    ("LI", 21),
    ("LT", 20),
    ("LU", 20),
    ("LV", 21),
    ("LY", 25),
    ("MC", 27),
    ("MD", 24),
    ("ME", 22),
    ("MK", 19),
    ("MN", 20),
    ("MR", 27),
    ("MT", 31),
    ("MU", 30),
    ("NI", 28),
    ("NL", 18),
    ("NO", 15),
    ("OM", 23),
    ("PK", 24),
    ("PL", 28),
    ("PS", 29),
    ("PT", 25),
    ("QA", 29),
    ("RO", 24),
    ("RS", 22),
    ("RU", 33),
    ("SA", 24),
    ("SC", 31),
    ("SD", 18),
    ("SE", 24),
    ("SI", 19),
    ("SK", 24),
    ("SM", 27),
    ("SO", 23),
    ("ST", 25),
    ("SV", 28),
    ("TL", 23),
    ("TN", 24),
    ("TR", 26),
    ("UA", 29),
    ("VA", 22),
    ("VG", 24),
    ("XK", 20),
    ("YE", 30),
];

/// US ABA routing transit number mod-10, weights (3,7,1) repeating over the
/// 9 digits. `d` MUST be exactly 9 digits.
pub(super) fn aba_mod10(d: &[u8]) -> bool {
    if d.len() != 9 {
        return false;
    }
    let sum = 3 * (d[0] + d[3] + d[6]) as u32
        + 7 * (d[1] + d[4] + d[7]) as u32
        + (d[2] + d[5] + d[8]) as u32;
    sum.is_multiple_of(10)
}

/// NPI Luhn check: prepend the fixed prefix 80840 and Luhn-validate the whole
/// 15-digit run (the standard CMS algorithm). `d` MUST be exactly 10 digits.
pub(super) fn npi_luhn(d: &[u8]) -> bool {
    if d.len() != 10 {
        return false;
    }
    let mut full: Vec<u8> = vec![8, 0, 8, 4, 0];
    full.extend_from_slice(d);
    luhn_ok(&full)
}

/// UK NHS number mod-11: weight the first 9 digits by 10..=2, take 11 − (Σ mod
/// 11) as the check digit (11 → 0, 10 → invalid), compare to the 10th digit.
/// `d` MUST be exactly 10 digits.
pub(super) fn nhs_mod11(d: &[u8]) -> bool {
    if d.len() != 10 {
        return false;
    }
    let sum: u32 = d[..9]
        .iter()
        .enumerate()
        .map(|(i, &v)| v as u32 * (10 - i as u32))
        .sum();
    let r = sum % 11;
    let check = 11 - r;
    let expected = match check {
        11 => 0,
        10 => return false, // 10 is not a valid NHS check digit
        c => c,
    };
    expected == d[9] as u32
}

pub(super) fn ipv4_octets_ok(m: &str) -> bool {
    m.split('.')
        .all(|o| o.parse::<u32>().map(|n| n <= 255).unwrap_or(false))
}

/// Mask every match of `re` for which `decide(s, match)` returns true, leaving
/// the rest of the text (and non-matches) byte-for-byte intact.
pub(super) fn mask_re(s: &str, re: &Regex, decide: impl Fn(&str, &regex::Match) -> bool) -> String {
    re.replace_all(s, |c: &Captures| {
        let m = c.get(0).unwrap();
        if decide(s, &m) {
            "<redacted>".to_string()
        } else {
            m.as_str().to_string()
        }
    })
    .into_owned()
}

// ---------------------------------------------------------------------------
// Crypto private keys that are bare hex / base58 — DEFAULT ON but proximity
// gated (except the self-anchoring Bitcoin WIF). A 64-hex run alone is
// indistinguishable from a SHA-256 / git blob, so ETH/XRP keys are masked ONLY
// when a wallet/secret keyword sits nearby. 100% regex + context, no ML.
// ---------------------------------------------------------------------------
pub(super) const CRYPTO_KWS: &[&str] = &[
    "private key",
    "private_key",
    "privatekey",
    "priv key",
    "privkey",
    "secret",
    "mnemonic",
    "seed phrase",
    "wallet",
];

pub(super) fn redact_context_crypto(s: &str) -> String {
    static WIF: OnceLock<Regex> = OnceLock::new();
    static ETH: OnceLock<Regex> = OnceLock::new();
    static XRP: OnceLock<Regex> = OnceLock::new();
    // Bitcoin WIF — `[5KL]` + 50–51 base58 chars. Distinctive enough to mask
    // without context.
    let wif = WIF.get_or_init(|| Regex::new(r"\b[5KL][1-9A-HJ-NP-Za-km-z]{50,51}\b").unwrap());
    let mut out = wif.replace_all(s, "<redacted>").into_owned();
    // Ethereum private key — `0x` + 64 hex. Proximity-gated (else every git
    // blob / SHA-256 would vanish).
    // Both remaining rules are proximity-gated on the same keywords, so one
    // absence check skips both scans. `\b0x[hex]{64}\b` and the XRP shape are
    // exactly the patterns a source tree or a log is full of.
    let crypto_ctx = haystack_has_context(&s.to_ascii_lowercase(), CRYPTO_KWS);
    let eth = ETH.get_or_init(|| Regex::new(r"\b0x[a-fA-F0-9]{64}\b").unwrap());
    if crypto_ctx {
        out = mask_re(&out, eth, |s, m| {
            ctx_near(s, m.start(), m.end(), CRYPTO_KWS)
        });
    }
    // Ripple/XRP family secret — `s`/`S` + 28–35 base58-ish. Very broad, so
    // context is mandatory.
    let xrp = XRP.get_or_init(|| Regex::new(r"\b[sS][a-zA-Z0-9]{28,35}\b").unwrap());
    if crypto_ctx {
        out = mask_re(&out, xrp, |s, m| {
            ctx_near(s, m.start(), m.end(), CRYPTO_KWS)
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Rule-based PII / PHI — DEFAULT ON. Presidio's 2-stage method, deterministic:
// regex candidate → checksum validate (hard accept/reject) → context boost.
// Ambiguous numeric-only types (ABA, NPI, NHS) require BOTH a valid checksum
// AND a nearby keyword; bank accounts require context; structurally-anchored
// types (IBAN mod-97, dashed SSN/ITIN) stand alone. No ML/NER — regex +
// arithmetic only. Cards are handled by `redact_pan_cvv`. IP / MAC are NOT
// here — they are ubiquitous in normal output, so they live in the opt-in
// `redact_optin_pii` pass instead.
// ---------------------------------------------------------------------------
pub(super) const ABA_KWS: &[&str] = &["aba", "routing", "rtn", "transit"];
pub(super) const NPI_KWS: &[&str] = &[
    "npi",
    "national provider",
    "provider id",
    "provider identifier",
];
pub(super) const NHS_KWS: &[&str] = &["nhs", "patient", "health"];
pub(super) const ACCT_KWS: &[&str] = &["account", "acct", "iban", "a/c"];

/// True when the digit run at `[start, end)` is a NUMBER, not a segment of a
/// longer dotted or hyphenated token.
///
/// `\b\d{8,17}\b` treats `.` and `-` as boundaries, so it happily matches the
/// `20260526` inside a package version `20260526.0-2`. Measured 2026-09-03 on
/// this machine's `pacman -Q`: one version in 1 533 came back `<redacted>.0-2`,
/// because the very next line is `accounts-qml-module` and the word "account"
/// inside a PACKAGE NAME opened the bank-account context gate 48 bytes away.
/// A listing of packages exists to carry its versions.
///
/// A real account number stands alone. It may end a sentence — `Account:
/// 12345678.` — so a trailing separator is fine as long as the token does not
/// CONTINUE past it into more characters.
pub(super) fn is_standalone_number(s: &str, start: usize, end: usize) -> bool {
    let cont = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let before = s[..start].chars().next_back();
    let after = s[end..].chars().next();
    let joined_left = matches!(before, Some('.') | Some('-'))
        && s[..start].chars().rev().nth(1).is_some_and(cont);
    let joined_right =
        matches!(after, Some('.') | Some('-')) && s[end..].chars().nth(1).is_some_and(cont);
    !(joined_left || joined_right || before.is_some_and(cont) || after.is_some_and(cont))
}

/// Could ANY match in `hay` clear a context gate built from `kws`?
///
/// The context-gated rules (ABA, NPI, NHS, bank account, ETH/XRP keys) scan the
/// whole text with a broad numeric or hex pattern and only then ask whether a
/// keyword sits within 48 bytes of each hit. Source code and logs are full of
/// digit runs and hex, so that is a full scan per rule to mask nothing —
/// measured at 49 ms for the PII pass alone on a 341 KB source file.
///
/// If the keyword appears nowhere in the text, `ctx_near` is false for every
/// possible match, so the rule provably masks nothing and can be skipped. This
/// is an equivalence, not a heuristic.
///
/// Callers pass the ORIGINAL text even when later rules run on a partly-masked
/// copy. That direction is the safe one: masking replaces spans with
/// `<redacted>` and can only ever REMOVE a keyword, so gating on the original
/// can let a rule run that would have found nothing — never the reverse.
pub(super) fn haystack_has_context(lower_hay: &str, kws: &[&str]) -> bool {
    kws.iter().any(|k| lower_hay.contains(k))
}
