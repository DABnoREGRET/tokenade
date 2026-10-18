//! **The redaction entry point, and argv.**
//!
//! What calls everything else, in order, and counts what was removed.
//! argv is separate because a secret is almost never in the same place
//! as its announcement there: `--token X` puts it in the NEXT word, and the programs
//! that accept a glued `-p<value>` are named one by one.

use super::*;

/// Literals that EVERY keyword-assignment rule requires, case-insensitively.
///
/// Not a heuristic: each of the seven rules is an alternation whose every branch
/// contains one of these substrings (`PGPASSWORD` → `pass`, `MYSQL_PWD` → `pwd`,
/// `REDISCLI_AUTH` → `auth`, `aws_session_token` → `token`, `api[_-]?key` →
/// `key`, …). A haystack containing none of them cannot match any rule, so
/// skipping is identical BY CONSTRUCTION. `every_keyword_rule_has_a_literal`
/// re-checks that property against the rule table, so adding a rule without one
/// fails the suite instead of quietly opening a hole.
pub(super) const KEYWORD_LITERALS: &[&str] = &["pass", "pwd", "secret", "token", "key", "auth"];

/// Input for [`warm`]: one block per gated pass in [`redact`]'s chain, so the
/// warm-up actually reaches the lazy regexes that sit BEHIND those gates
/// instead of stopping at the two big sets. Every value is a documentation
/// dummy — the Visa test PAN, an RFC 5737 address, an all-zero key — never a
/// real credential.
///
/// `warm_corpus_still_trips_every_gate` fails if a pass stops matching it,
/// which is the only way this silently degrades into warming nothing.
pub(super) const WARM_BLOCKS: &[&str] = &[
    "Authorization: Bearer warmup\nBasic d2FybXVwdXNlcjp3YXJtdXBwYXNzd29yZA==\n",
    "name,value,domain\nsession,warm,example.com\n",
    "{\"name\":\"session\",\"value\":\"warm\",\"domain\":\"example.com\"}\n",
    "apiVersion: v1\nkind: Secret\nmetadata:\n  name: warmup\ndata:\n  token: d2FybXVwLXNlY3JldC12YWx1ZQ==\n",
    "private key: 0xa3f1c9d20e47b856f0d3128e6b94af7205cd8e13b6a2f490c7de5b18034ea62f\n",
    "card 4111111111111111 cvv 123\n",
    "IBAN GB82WEST12345698765432 SSN 123-45-6789 ABA 021000021 NPI 1234567893\n",
    "warmup=k3Jd8vQz1LmXbR7pWnTaY5cHfEoU2sGiA0ZyMqNrB6tK\n",
];

/// The one warm block for the OPT-IN PII pass, kept apart because that pass is
/// default-OFF: it is appended to the corpus regardless, which costs nothing
/// when the pass is disabled (it never runs, so it never compiles) and warms it
/// for the users who did turn it on.
pub(super) const WARM_BLOCK_OPTIN_PII: &str =
    "email warm@example.com phone +1-202-555-0100 ip 192.0.2.1 mac 00:11:22:33:44:55\n";

/// Compile the two big regex sets ahead of the first `redact` call.
///
/// Every hooked tool call is a FRESH process, so the `OnceLock`s below are not
/// amortised across calls the way a long-lived server would amortise them: the
/// provider `RegexSet` (98 patterns) and the keyword-assignment rules cost ~8 ms
/// to build, EVERY command, and they were being built on the critical path
/// between the child exiting and its output being printed.
///
/// The work is unavoidable, but the *wait* is not: called from a background
/// thread while the wrapped command runs, it overlaps with a child process that
/// almost always outlives it. Purely a warm-up — `get_or_init` makes a racing
/// main thread block exactly as long as it does today and never longer, and a
/// process that exits before the thread finishes loses nothing.
pub fn warm() {
    // The two independent sets, built concurrently: 6.3 ms and 2.9 ms of
    // pure regex compilation with no data between them, so serialising them
    // just adds the smaller one to the wait.
    // ~112 independent `Regex::new` calls plus the keyword set: no data flows
    // between any of them, so compiling them on one thread just adds them up.
    // Striped across a few threads instead (`OnceLock` makes a lost race a
    // no-op), which is what keeps the warm-up shorter than the shortest
    // command worth wrapping.
    let lanes = std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 4))
        .unwrap_or(1);
    let mut handles = Vec::with_capacity(lanes);
    handles.push(std::thread::spawn(|| {
        let _ = keyword_assignment_rules();
    }));
    let ps = patterns();
    for lane in 1..lanes {
        handles.push(std::thread::spawn(move || {
            let ps = patterns();
            for i in (lane..ps.raw.len()).step_by(lanes) {
                let _ = ps.rule(i);
            }
        }));
    }
    for i in (0..ps.raw.len()).step_by(lanes) {
        let _ = ps.rule(i);
    }
    for h in handles {
        let _ = h.join();
    }
    // The passes AFTER the two big sets carry their own lazy regexes, and most
    // are gated behind a cheap signature check — so warming them means handing
    // `redact` an input that trips every gate. One line per pass, in chain
    // order; every value here is a documentation-grade dummy (test-card number,
    // RFC 5737 address, all-zero key), never a real credential.
    let _ = redact(&(WARM_BLOCKS.concat() + WARM_BLOCK_OPTIN_PII));
}

/// How many masks the text carries. `<redacted` is a prefix on purpose: it
/// covers both the plain `<redacted>` value form and the cookie-table
/// `<redacted N chars>` form, the same way [`redact_counting`] does.
pub(super) fn marks(s: &str) -> usize {
    s.matches("<redacted").count()
}

/// Credit a pass with whatever it masked, when anyone asked.
///
/// The ledger used to record only a daily TOTAL, so a rule that started
/// firing wrongly showed up as an anonymous spike. That is not hypothetical:
/// the entropy sweep was masking Rust test names — `web::tests::a11y_…` — and
/// the number that would have named it in seconds instead said `n: 111`.
///
/// `None` is the ordinary path (bare [`redact`]) and costs one branch per
/// pass; only model-bound text pays the marker scans. Measured on 1 MB of log
/// text: 1.317 s tallied against 1.319 s plain — eleven substring scans do not
/// register beside the regex passes they are counting.
pub(super) fn credit(
    name: &'static str,
    out: &str,
    seen: &mut usize,
    tally: &mut Option<&mut Vec<(&'static str, usize)>>,
) {
    let Some(t) = tally else { return };
    let now = marks(out);
    if now > *seen {
        t.push((name, now - *seen));
    }
    *seen = now;
}

/// Replace every secret-shaped value in `s` with `<redacted>`. Idempotent:
/// calling on already-redacted text leaves it unchanged. Never panics —
/// uses pre-compiled regex on a fixed set of patterns.
///
/// Passes run specific → generic so precise rules win and the generic
/// entropy sweep never re-processes an already-`<redacted>` value:
///   1. fixed provider/format/keyword regexes (`patterns()`)
///   2. structural cookie tables (csv/json)
///   3. K8s `kind: Secret` base64 data (strict gate)
///   4. ETH/XRP private keys                  — DEFAULT ON, proximity-gated
///   5. PAN + CVV (Luhn-validated)            — opt-out `TOKENADE_NO_REDACT_PAN`
///   6. rule-based PII/PHI: IBAN/SSN/ITIN/ABA/NPI/NHS/bank account
///                                            — DEFAULT ON (checksum+context rules, no ML)
///   7. email + phone + IP + MAC              — OPT-IN `TOKENADE_REDACT_PII=1` (default OFF)
///   8. generic high-entropy secrets (LAST)   — opt-out `TOKENADE_NO_REDACT_ENTROPY`
pub fn redact(s: &str) -> String {
    redact_inner(s, &mut None)
}

/// [`redact`] plus, per pass, how many values it masked. Only the metered
/// entry point ([`redact_for_model`]) asks for the breakdown.
pub fn redact_counting_tallied(s: &str) -> (String, usize, Vec<(&'static str, usize)>) {
    let mut tally = Vec::new();
    let before = marks(s);
    let out = redact_inner(s, &mut Some(&mut tally));
    let n = marks(&out).saturating_sub(before);
    (out, n, tally)
}

pub(super) fn redact_inner(s: &str, tally: &mut Option<&mut Vec<(&'static str, usize)>>) -> String {
    let mut seen = marks(s);
    let mut out = crate::profile::timed("rd:providers", || apply_provider_patterns(s));
    credit("providers", &out, &mut seen, tally);
    // Keyword assignments run here, right where they used to sit in the list,
    // but with a veto on values that are plainly code. See `value_is_code`.
    out = crate::profile::timed("rd:keyword", || redact_keyword_assignments(&out));
    credit("keyword", &out, &mut seen, tally);
    // Structural cookie-export redaction — applied UNCONDITIONALLY at this
    // universal boundary. The csv/json compactors also scrub cookies, but they
    // pass the ORIGINAL text through untouched when the input is too small to
    // compact (anti-inflation guard) — exactly the DevTools-export case we care
    // about. Both helpers are byte-for-byte no-ops on non-cookie input (gated by
    // the cookie signature), so ordinary text is never disturbed.
    // Bare `Bearer <x>` / `Basic <x>` outside an Authorization header. Shape
    // gated — see the function for why.
    out = crate::profile::timed("rd:auth", || redact_bare_auth_schemes(&out));
    credit("auth", &out, &mut seen, tally);
    // XML/HTML elements: the tag is the key and the text is the value, a shape
    // no assignment rule can see.
    out = crate::profile::timed("rd:xml", || redact_xml_credential_elements(&out));
    credit("xml", &out, &mut seen, tally);
    out = crate::profile::timed("rd:csvcookie", || crate::redact::redact_cookie_table(&out));
    credit("csv-cookie", &out, &mut seen, tally);
    out = crate::profile::timed("rd:jsoncookie", || crate::redact::redact_cookie_json(&out));
    credit("json-cookie", &out, &mut seen, tally);
    // Structural: Kubernetes Secret manifests (strict `kind: Secret` gate).
    out = crate::profile::timed("rd:k8s", || redact_k8s_secret(&out));
    credit("k8s", &out, &mut seen, tally);
    // Crypto private keys that are bare hex/base58 (ETH / XRP) — default ON but
    // proximity-gated on a `private key`/`secret`/`mnemonic` keyword, because a
    // 64-hex run alone is indistinguishable from a SHA-256 / git blob.
    out = crate::profile::timed("rd:crypto", || redact_context_crypto(&out));
    credit("crypto", &out, &mut seen, tally);
    // Validated / heuristic passes, gated by env opt-outs (hot-path FP hatch).
    if !opt_out("TOKENADE_NO_REDACT_PAN") {
        out = crate::profile::timed("rd:pan", || redact_pan_cvv(&out));
        credit("pan", &out, &mut seen, tally);
    }
    // Rule-based PII / PHI — DEFAULT ON. Ambiguous types carry a mandatory
    // checksum (ABA mod-10, NPI Luhn, NHS mod-11) and/or a required context
    // keyword (bank account / passport / driver-licence / IP / MAC), so FP is
    // held down by the RULES, not by an opt-in. IBAN/SSN/ITIN are structurally
    // self-anchoring. 100% regex + arithmetic — no ML/NER anywhere.
    out = crate::profile::timed("rd:pii", || redact_pii_structured(&out));
    credit("pii", &out, &mut seen, tally);
    // Email / phone / IP / MAC are default-OFF PII: they appear constantly in
    // ordinary output (`git log`, `netstat`, `docker inspect`, `ifconfig`,
    // logs), so masking them by default degrades the output for no security
    // win the user asked for. Opt in with TOKENADE_REDACT_PII (umbrella;
    // legacy alias TOKENADE_REDACT_EMAIL_PHONE still honoured).
    if opt_in("TOKENADE_REDACT_PII") || opt_in("TOKENADE_REDACT_EMAIL_PHONE") {
        out = redact_optin_pii(&out);
        credit("pii-optin", &out, &mut seen, tally);
    }
    // Entropy runs LAST so every specific rule above has already claimed its
    // value (leaving `<redacted>`, which is too short/low-class to re-trigger).
    if !opt_out("TOKENADE_NO_REDACT_ENTROPY") {
        out = crate::profile::timed("rd:entropy", || redact_entropy(&out));
        credit("entropy", &out, &mut seen, tally);
    }
    out
}

/// Redact every element of an argv vector. The first element (the program
/// name) is left untouched.
/// Redaction for a caller that can only see ONE LINE AT A TIME.
///
/// [`redact`] is given the whole text, so its rules can span lines — and the
/// one that matters most does: a private key is a BEGIN marker, a body and an
/// END marker on separate lines, and the rule erases the block. A caller that
/// streams (because the file may be gigabytes) hands the redactor one line at
/// a time, and that rule can then never fire.
///
/// Measured 2026-09-03 on a fixture holding one RSA key: `tokenade redact`
/// wrote `<redacted-private-key>` for the BEGIN line and then wrote the key
/// body and the END line through, unchanged, into the `.redacted` file it
/// reports as done — while the same bytes through the model path came back
/// with the whole block replaced. The verb whose only job is to leave no
/// secret in a file left the secret in the file.
///
/// The block is recognised on the RAW line, not on the redacted output: an
/// already-redacted file carries the `<redacted-private-key>` marker and no
/// BEGIN line, so re-running changes nothing, which is the idempotence the
/// command's help promises.
#[derive(Debug, Default)]
pub struct LineRedactor {
    in_key_block: bool,
}

impl LineRedactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// The line to write, or `None` when it belongs to a block whose marker
    /// has already been emitted.
    pub fn line(&mut self, line: &str) -> Option<String> {
        if self.in_key_block {
            if line.contains("-----END") {
                self.in_key_block = false;
            }
            return None;
        }
        let upper = line.to_ascii_uppercase();
        let opens = upper.contains("-----BEGIN") && upper.contains("PRIVATE KEY");
        if opens && !upper.contains("-----END") {
            self.in_key_block = true;
        }
        Some(redact(line))
    }
}

pub fn redact_argv(argv: &[String]) -> Vec<String> {
    if argv.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(argv.len());
    out.push(argv[0].clone());
    for arg in &argv[1..] {
        out.push(redact(arg));
    }
    // Also handle the case `--password VALUE` where VALUE is the *next*
    // element: scan pairwise and replace VALUE.
    redact_paired_flags(&mut out);
    out
}

/// Long-form paired flags that ALWAYS carry secrets regardless of
/// the program — every tool that takes `--password` etc. takes a
/// credential there.
pub(super) const PAIRED_FLAG_TRIGGERS: &[&str] = &[
    "--password",
    "--token",
    "--secret",
    "--api-key",
    "--api_key",
    "--apikey",
    "--auth",
];

/// Short flags `-p` / `-u` are ambiguous: secret-bearing for
/// mysql/redis-cli/sshpass, but package / project selectors for
/// cargo/pnpm/yarn/git. Only redact the value when argv[0] is in
/// this allow-list. Falsely redacting `cargo test -p tokenade-core`
/// would lose a non-secret package name and hurt log debugging.
pub(super) const SHORT_PAIRED_FLAG_SECRET_PROGS: &[&str] = &[
    "mysql",
    "mariadb",
    "mysqldump",
    "redis-cli",
    "pg_dump",
    "pg_restore",
    "psql",
    "sshpass",
    "openssl",
    "ldapsearch",
    "ldapadd",
    "smbclient",
];

pub(super) fn redact_paired_flags(args: &mut [String]) {
    if args.is_empty() {
        return;
    }
    // Track DANGEROUS short-flag context across the whole argv, not
    // just argv[0]. Production leak: `tokenade wrap 'sshpass
    // -p PWD ssh ...'` puts `tokenade` at argv[0] (not in the
    // allow-list) but `sshpass` later in the stream. Without this
    // scan, `-p PWD` is missed and the password lands in debug.log.
    // Other wrappers with the same hazard: `bash -c "sshpass …"`,
    // `sh -c "…"`, `env … sshpass …`, any pipeline beginning with
    // a benign program.
    //
    // The state machine: walk left-to-right, set `dangerous` to true
    // whenever a token matches SHORT_PAIRED_FLAG_SECRET_PROGS (bare
    // or quoted/with-path). Once flipped on, the rest of the argv
    // can use short paired flags. We never flip back to false — the
    // sshpass command may be nested deep in a shell pipeline and we
    // don't want to track shell-grammar termination here.
    fn strip_quotes_and_path(s: &str) -> String {
        let mut t = s.trim_start_matches(['\'', '"']);
        t = t.trim_end_matches(['\'', '"']);
        // Strip leading shell escape like `\'sshpass` (bash $'\'...').
        t = t.trim_start_matches('\\').trim_start_matches('\'');
        t.rsplit(['/', '\\'])
            .next()
            .unwrap_or(t)
            .to_ascii_lowercase()
    }
    let mut short_flags_dangerous = SHORT_PAIRED_FLAG_SECRET_PROGS
        .iter()
        .any(|p| *p == strip_quotes_and_path(&args[0]));
    let n = args.len();
    let mut i = 0;
    while i < n {
        // Flip dangerous ON when we see a known-secret-bearing prog
        // anywhere in the argv. Quoting/path-stripping reuses the
        // same helper as argv[0].
        if !short_flags_dangerous {
            let bare = strip_quotes_and_path(&args[i]);
            if SHORT_PAIRED_FLAG_SECRET_PROGS.iter().any(|p| *p == bare) {
                short_flags_dangerous = true;
            }
        }
        let triggered = {
            let a = &args[i];
            PAIRED_FLAG_TRIGGERS
                .iter()
                .any(|t| a.eq_ignore_ascii_case(t))
                || (short_flags_dangerous
                    && (a.eq_ignore_ascii_case("-p") || a.eq_ignore_ascii_case("-u")))
        };
        if triggered && i + 1 < n {
            // Don't redact if next arg looks like another flag (`--foo`).
            if !args[i + 1].starts_with('-') {
                args[i + 1] = "<redacted>".into();
            }
        }
        i += 1;
    }
}
