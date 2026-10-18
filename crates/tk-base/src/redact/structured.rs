//! **The secret stored in a STRUCTURE.**
//!
//! A JSON key, an XML element, a bare `Authorization` header: the value does not
//! look like anything in particular, it is its PLACE that gives it away. The
//! opposite rule to entropy's, and that is why they live apart.

use super::*;

pub(super) fn redact_pii_structured(s: &str) -> String {
    static IBAN: OnceLock<Regex> = OnceLock::new();
    static SSN: OnceLock<Regex> = OnceLock::new();
    static ABA: OnceLock<Regex> = OnceLock::new();
    static NPI: OnceLock<Regex> = OnceLock::new();
    static NHS: OnceLock<Regex> = OnceLock::new();
    static ACCT: OnceLock<Regex> = OnceLock::new();
    // One lowercase copy for all four context gates below. See
    // `haystack_has_context` for why the ORIGINAL text is the right input.
    let lower = s.to_ascii_lowercase();

    // IBAN — country(2) + check(2) + 11–30 alnum, mod-97 validated (strong,
    // stands alone).
    let iban = IBAN.get_or_init(|| Regex::new(r"\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b").unwrap());
    let mut out = mask_re(s, iban, |_, m| {
        validated(
            iban_mod97(m.as_str()) && iban_country_length_ok(m.as_str()),
            false,
        )
    });

    // US SSN + ITIN — dashed form only (bare 9-digit is pure FP). Structural,
    // self-anchoring. ITIN (`9xx-[7-9]x-xxxx`) is a subset of this shape.
    let ssn = SSN.get_or_init(|| Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap());
    out = mask_re(&out, ssn, |_, _| validated(true, false));

    // ABA routing — leading digit 0/1/2/3/6/7/8, 9 digits, mod-10(3,7,1).
    // Checksum AND context both required (random 9-digit passes mod-10 ~10%).
    if haystack_has_context(&lower, ABA_KWS) {
        let aba = ABA.get_or_init(|| Regex::new(r"\b[0123678]\d{8}\b").unwrap());
        out = mask_re(&out, aba, |s, m| {
            let d = digits(m.as_str());
            validated(
                aba_mod10(&d) && ctx_near(s, m.start(), m.end(), ABA_KWS),
                false,
            )
        });
    }

    // NPI — 10 digits starting 1/2, Luhn(80840-prefixed). Checksum + context.
    if haystack_has_context(&lower, NPI_KWS) {
        let npi = NPI.get_or_init(|| Regex::new(r"\b[12]\d{9}\b").unwrap());
        out = mask_re(&out, npi, |s, m| {
            let d = digits(m.as_str());
            validated(
                npi_luhn(&d) && ctx_near(s, m.start(), m.end(), NPI_KWS),
                false,
            )
        });
    }

    // UK NHS number — 3-3-4 grouping (optional single spaces), mod-11.
    // Checksum + context (bare 10-digit run overlaps phone numbers).
    if haystack_has_context(&lower, NHS_KWS) {
        let nhs = NHS.get_or_init(|| Regex::new(r"\b\d{3}\s?\d{3}\s?\d{4}\b").unwrap());
        out = mask_re(&out, nhs, |s, m| {
            let d = digits(m.as_str());
            validated(
                nhs_mod11(&d) && ctx_near(s, m.start(), m.end(), NHS_KWS),
                false,
            )
        });
    }

    // Generic bank account number — 8–17 digits, CONTEXT-ONLY (no checksum
    // exists), else the FP would be enormous. Runs LAST; earlier numeric types
    // are already `<redacted>` (digit-free) so they can't re-match.
    if haystack_has_context(&lower, ACCT_KWS) {
        let acct = ACCT.get_or_init(|| Regex::new(r"\b\d{8,17}\b").unwrap());
        out = mask_re(&out, acct, |s, m| {
            validated(
                false,
                is_standalone_number(s, m.start(), m.end())
                    && ctx_near(s, m.start(), m.end(), ACCT_KWS),
            )
        });
    }

    out
}

// ── Redaction metering ────────────────────────────────────────────────────
// The dashboard (terminal + HTML + --json) surfaces "how many secrets were
// redacted before reaching the model" per period. Every MODEL-BOUND
// redaction site calls [`redact_for_model`] instead of bare [`redact`];
// log/label scrubs (gain.jsonl labels, debug.log command lines) keep the
// bare call so the counter only counts values an AI would otherwise have
// received.

/// `Bearer <value>` / `Basic <value>` written WITHOUT an `Authorization:`
/// header around them (the header form is rule C7, which stays unconditional).
///
/// Gated on the shape of the value, because as plain regexes these two rules
/// redacted English. `swagger.io/docs/specification/v3_0/basic-structure/`
/// came back titled `# Basic <redacted>` — the page is "Basic Structure", and
/// the word after `Basic` was masked as a credential. "Basic Usage", "Basic
/// Concepts", "Bearer token" are the same sentence in most API documentation,
/// which is exactly the corpus an agent reads.
///
/// A credential is `base64(user:pass)` or an opaque token: at least 16
/// characters, and NOT a word — either it carries a digit or base64 padding,
/// or it has an uppercase letter somewhere other than the first position.
/// `Authentication` (14, capitalised, all letters) is prose; `dXNlcjpwYXNz`
/// and `YWRtaW46czNjcmV0` are not. Erring toward prose here is safe: an
/// `Authorization:` header — how a credential actually travels — is redacted
/// by C7 regardless of shape.
/// XML/HTML ELEMENT text whose tag names a credential:
/// `<password>hunter2</password>`, `<client-secret>…</client-secret>`,
/// `<apiKey>…</apiKey>`.
///
/// Every other rule in this module reads an ASSIGNMENT — `key = value`,
/// `key: value`, `--flag value`. XML has no assignment: the name is a tag and
/// the value is the text between the tags, so a Maven `settings.xml`, a .NET
/// `app.config` or a Spring `applicationContext.xml` walked through every pass
/// untouched. Measured 2026-08-28 with a sweep of twenty real credential
/// shapes: this was one of the two that came back verbatim.
///
/// Two shape gates, because a tag name is not enough:
///
///   * the value must be FOLLOWED by `</` — a real closing tag. Without it
///     `List<Token> tokens = new ArrayList<>();` matches `<Token>` and masks
///     the rest of the statement. The regex crate has no lookahead, so the
///     check lives in the closure, exactly like the `(`-veto in the entropy
///     pass. A CLOSING tag cannot match at all: `/` is not in the tag class.
///   * a placeholder value (`${…}`, `{{…}}`, `<!-- … -->`) is left alone, the
///     same predicate the provider table uses.
///
/// The keyword list is deliberately NARROWER than the entropy pass's: that one
/// carries a bare `key` and a bare `auth`, and `<key>CFBundleName</key>` is
/// every second line of an Apple plist. A rule that eats plists to catch
/// `<apiKey>` is not worth having.
pub(super) fn redact_xml_credential_elements(s: &str) -> String {
    if !s.contains('<') {
        return s.to_string();
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r#"(?i)(<[\w.:-]*(?:password|passwd|pwd|secret|credential|api[_\-]?key|apikey|access[_\-]?key|auth[_\-]?token|token)[\w.:-]*>)([^<\r\n]+)"#,
        )
        .expect("static regex")
    });
    re.replace_all(s, |c: &Captures| {
        let whole = c.get(0).expect("group 0 always exists");
        let open = &c[1];
        let value = &c[2];
        if !s[whole.end()..].starts_with("</") {
            return whole.as_str().to_string();
        }
        if value_is_placeholder(value.trim()) {
            return whole.as_str().to_string();
        }
        format!("{open}<redacted>")
    })
    .into_owned()
}

pub(super) fn redact_bare_auth_schemes(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(Bearer|Basic)(\s+)([^\s"'`<>]+)"#).expect("static regex")
    });
    re.replace_all(s, |c: &regex::Captures| {
        let (scheme, gap, value) = (&c[1], &c[2], &c[3]);
        // `--password-store=basic --log-file=/…`: `basic` is the VALUE of a
        // flag and what follows is the NEXT flag — every Chromium/Electron
        // process in `ps auxww` lost its next argument (4 on this machine,
        // 2026-10-03). A credential never opens on `--`.
        if value.starts_with("--") {
            return c[0].to_string();
        }
        if looks_like_credential(value) {
            format!("{scheme}{gap}<redacted>")
        } else {
            c[0].to_string()
        }
    })
    .into_owned()
}

pub(super) fn looks_like_credential(v: &str) -> bool {
    if v.starts_with("<redacted") {
        return false; // idempotent
    }
    if v.chars().count() < 16 {
        return false;
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_' | '.'))
    {
        return false;
    }
    let non_alpha = v.chars().any(|c| !c.is_ascii_alphabetic());
    let inner_upper = v.chars().skip(1).any(|c| c.is_ascii_uppercase());
    non_alpha || inner_upper
}

/// [`redact`] + how many secret values THIS pass masked. Counted as the
/// growth in `<redacted` markers (covers both the `<redacted>` value form
/// and the cookie-table `<redacted N chars>` form), so re-scrubbing
/// already-clean text counts 0 — same idempotence as `redact` itself.
pub fn redact_counting(s: &str) -> (String, usize) {
    let out = redact(s);
    let n = out
        .matches("<redacted")
        .count()
        .saturating_sub(s.matches("<redacted").count());
    (out, n)
}
