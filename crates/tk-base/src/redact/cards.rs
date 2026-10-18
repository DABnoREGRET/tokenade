//! Payment card numbers, and the checks that keep an order id from
//! looking like one.
//!
//! A 16-digit run is not a PAN unless it passes Luhn AND its issuer prefix
//! is one that exists — both, because either alone has a false-positive rate
//! that eats real output.

use super::*;

// ---------------------------------------------------------------------------
// H — Payment card PAN (Luhn-validated) + CVV (by proximity).
// ---------------------------------------------------------------------------
pub(super) fn luhn_ok(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    let mut alt = false;
    for &d in digits.iter().rev() {
        let mut v = d as u32;
        if alt {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
        alt = !alt;
    }
    sum.is_multiple_of(10)
}

/// Major-industry IIN gate so random Luhn-valid IDs are far less likely to
/// match: Visa `4`, Mastercard `51–55` / `2221–2720` (checked to 2 digits),
/// Amex `34`/`37`, Discover/other `6`.
pub(super) fn plausible_iin(d: &[u8]) -> bool {
    match d {
        [4, ..] | [6, ..] => true,
        [3, 4, ..] | [3, 7, ..] => true,
        [5, b, ..] if (1..=5).contains(b) => true,
        [2, b, ..] if (2..=7).contains(b) => true,
        _ => false,
    }
}

/// A separated run of digits is only a card if it is GROUPED like one.
///
/// The regex accepts any single space/dash between digits, so a line of plain
/// numbers — `20:26 3179168 2954167`, a timestamp and two token counts — reads
/// as one 16-digit run. It passed the IIN gate on `26` and, one time in ten, it
/// passes Luhn too: that exact line was served to an agent as
/// `20:****-****-****-4167` (OBSERVED, in this product's own output). A
/// mistaken mask is silent data loss — the reader cannot tell a redacted number
/// from a number that was never there.
///
/// Every real rendering groups in 3–6 digits with ONE separator throughout:
/// `4111 1111 1111 1111`, `4111-1111-1111-1111`, Amex `3782 822463 10005`,
/// Diners `3056 930902 5904`. `2 7 7` is not a card, and neither is `7 4 2 2`,
/// which is the shape real captured output actually contains (MEASURED: 15 of
/// 16 separated candidates across 61 MB of stashed command output are grouped
/// in a shape no card has ever been written in; the 16th is `4 4 4 4`).
///
/// Unseparated runs are unaffected — that is the form a leak usually takes.
pub(super) fn card_shaped(m: &str) -> bool {
    let seps: Vec<char> = m.chars().filter(|c| *c == ' ' || *c == '-').collect();
    if seps.is_empty() {
        return true;
    }
    if seps.iter().any(|c| *c != seps[0]) {
        return false;
    }
    m.split([' ', '-'])
        .all(|g| !g.is_empty() && (3..=6).contains(&g.len()))
}

pub(super) fn redact_pan_cvv(s: &str) -> String {
    static PAN: OnceLock<Regex> = OnceLock::new();
    let pan = PAN.get_or_init(|| {
        // 13–19 digits with optional single space/dash group separators.
        // `\b…\b` prevents matching *inside* a longer numeric ID (a 20-digit
        // run has no interior boundary that yields a 13–19 slice).
        Regex::new(r"\b\d(?:[ \-]?\d){12,18}\b").unwrap()
    });
    // Cheap pre-filter guards ONLY the (heavier) PAN scan — a 13-digit run is
    // impossible in a shorter string. CVV still runs (it needs ~7 chars).
    let out: std::borrow::Cow<str> = if s.len() >= 13 && s.bytes().any(|b| b.is_ascii_digit()) {
        pan.replace_all(s, |c: &Captures| {
            let m = c.get(0).unwrap().as_str();
            let digits: Vec<u8> = m
                .bytes()
                .filter(u8::is_ascii_digit)
                .map(|b| b - b'0')
                .collect();
            if !(13..=19).contains(&digits.len())
                || !card_shaped(m)
                || !plausible_iin(&digits)
                || !luhn_ok(&digits)
            {
                return m.to_string();
            }
            let last4: String = digits[digits.len() - 4..]
                .iter()
                .map(|d| (d + b'0') as char)
                .collect();
            format!("****-****-****-{last4}")
        })
    } else {
        std::borrow::Cow::Borrowed(s)
    };
    static CVV: OnceLock<Regex> = OnceLock::new();
    let cvv = CVV.get_or_init(|| {
        Regex::new(
            r"(?i)\b(cvv2?|cvc2?|card[\s_\-]*verification(?:[\s_\-]*(?:value|code|number))?)\b(\s*[:=]?\s*)\d{3,4}\b",
        )
        .unwrap()
    });
    cvv.replace_all(&out, "${1}${2}<redacted>").into_owned()
}

// ---------------------------------------------------------------------------
// Shared deterministic helpers for the crypto + PII/PHI passes (no ML/NER):
//   * `digits`        — extract a value's digit run for checksum arithmetic
//   * `window_lower`  — lower-cased text window around a match (context probe)
//   * `ctx_near`      — true when a keyword sits inside that window
//   * `validated`     — Presidio-style 2-stage accept (checksum ∨ context)
// ---------------------------------------------------------------------------
pub(super) fn digits(s: &str) -> Vec<u8> {
    s.bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect()
}
