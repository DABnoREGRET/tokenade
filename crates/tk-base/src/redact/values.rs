//! **Is this value a secret, or only a word that looks like one?**
//!
//! Half of the difficulty of redaction. A port followed by a path, a
//! file path, a dotted identifier, a placeholder
//! (`<your-key-here>`) are not secrets — and redacting them makes the
//! output unusable without protecting anything.

use super::*;

/// The provider/format rules, each `Regex` compiled on first use.
///
/// **This is not a `RegexSet`, and the difference was measured.** A triage
/// automaton was tried on 2026-08-04 and reverted the same day: its reasoning
/// was right about the OUTPUT (a rule the set does not report cannot match)
/// and silent about the COST. On 200 KB of base64, `RegexSet::matches` took
/// **374 ms** against **2,1 ms** for calling `is_match` on every rule in turn —
/// ~180x, and linear at ~2 µs/byte, so a 1 MB file spent ~2 s in redaction.
/// The original benchmark missed it by measuring short inputs only.
///
/// So the loop below runs every rule, and the only thing `OnceLock` buys is
/// that a rule is compiled once per process rather than once per call. Do not
/// reintroduce a set-based prefilter without benchmarking a LARGE haystack.
///
/// Running each rule in turn is safe for the same reason the set was: redaction
/// is idempotent (`redact(redact(x)) == redact(x)`, asserted in this module), so
/// inserting `<redacted>` cannot create a secret shape that was not already
/// there. `same_output_as_the_eager_loop` re-proves the equivalence over the
/// corpus.
pub(super) struct PatternSet {
    pub(super) raw: Vec<(&'static str, &'static str)>,
    pub(super) compiled: Vec<OnceLock<Regex>>,
}

impl PatternSet {
    /// The regex for rule `i`, compiled on first use.
    pub(super) fn rule(&self, i: usize) -> (&Regex, &'static str) {
        let re = self.compiled[i]
            .get_or_init(|| Regex::new(self.raw[i].0).expect("pattern is a literal in this file"));
        (re, self.raw[i].1)
    }
}

/// Stage 1 of [`redact`]: the 80 provider/format rules.
///
/// Split out so the triage can be proved equivalent to the eager loop it
/// replaced, in isolation from the seven other passes — see
/// `triage_is_equivalent`.
/// The value a rule is about to mask is a PLACEHOLDER, not a secret.
///
/// Shared by the provider pass and the keyword pass. Vetoing it in one and not
/// the other buys nothing: measured, `export OPENAI_API_KEY={master_key}` simply
/// moved from one tally to the other and the agent saw the same `<redacted>`.
///
/// `$VAR`, `{var}`, `<your key>`, `%s`: a variable reference or a fill-me-in
/// marker, which is what documentation and shell snippets are made of. Measured
/// 2026-08-28 over 1 993 third-party source files, this rule family masked 183
/// values on code holding no secret at all, and these are its shapes:
///
/// ```text
/// export APORIO_API_KEY=<your key>
/// -H "X-APORIA-API-KEY: $APORIO_API_KEY"
/// print(f"    export OPENAI_API_KEY={master_key}")
/// ```
///
/// No credential begins with any of these four bytes: base64 is `A-Za-z0-9+/=`,
/// base64url swaps in `-_`, hex is hex, and every provider prefix is
/// alphanumeric with `-`/`_`. Same argument the keyword and entropy passes
/// already use for their own vetos — this is the third pass, which had no place
/// to put one until now.
/// The value with its surrounding QUOTES removed, and nothing else.
///
/// Every veto in the keyword pass reads the value the regex captured, and the
/// quoted alternative in those rules captures the quotes with it. So
/// `"api_key": "null"` reached each clause as `"null"` — with a leading quote
/// no veto can match past — and JSON, Python and TypeScript are where the
/// quoted form lives. Measured 2026-08-28 over 2 000 third-party source files:
/// 203 of the 244 keyword masks carry a quoted value. Stripped ONCE here, so
/// the clauses cannot disagree about what the value is.
pub(super) fn unquoted(value: &str) -> &str {
    let v = value.trim_end_matches([',', ';', ')', '}']).trim();
    let b = v.as_bytes();
    if v.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        return &v[1..v.len() - 1];
    }
    v
}

/// The shell's WORKING DIRECTORY, not a password: `PWD=/Users/me/project`.
///
/// `pwd` is a password keyword (`;Pwd=…;` in a DSN, `MYSQL_PWD=`), and every
/// `env`, `printenv` and `set` prints `PWD=` — measured 2026-10-02 over the
/// corpus fixtures and a real `set`: the one line saying where the shell
/// stands came back `PWD=<redacted>`, on every environment dump. Spared only
/// in the shell's own spelling: the key exactly `PWD` (a DSN writes `Pwd`
/// as often as `PWD`, so the next condition carries the weight), NOT after a
/// `;` (inside a connection string it is a password), and a value that is an
/// absolute path.
pub(super) fn value_is_working_directory(pre: &str, value: &str) -> bool {
    let key = pre.trim_start_matches([' ', '\t', '\r', '\n']);
    if key != "PWD=" {
        return false;
    }
    let mut c = value.chars();
    match (c.next(), c.next(), c.next()) {
        (Some('/' | '~'), _, _) => true,
        (Some(d), Some(':'), Some('\\' | '/')) => d.is_ascii_alphabetic(),
        _ => false,
    }
}

pub(super) fn value_is_placeholder(value: &str) -> bool {
    let v = value.trim_start();
    v.starts_with('$') || v.starts_with('{') || v.starts_with('<') || v.starts_with('%')
}

/// The value is an IDENTIFIER or a dotted/slashed path of identifiers, and
/// carries no digit — a variable being passed through, not a credential.
///
/// Used by the KEYWORD pass only, and that limit was measured rather than
/// assumed: applied to the provider table as well it unmasked real vendor
/// tokens — `hf_…`, `npm_…`, a Sentry token — because those carry `_` as a
/// prefix separator and can be all letters, which is exactly the shape this
/// predicate accepts. The provider table matches on a KNOWN prefix and is
/// confident by construction; the keyword rules match a generic `api_key =`
/// and are where an identifier actually turns up.
///
/// Measured 2026-08-28 on 1 993 third-party source files, this is the LARGEST
/// remaining class of masked lines: roughly a third of them are
///
/// ```text
/// api_key=self.api_key,
/// api_key=litellm_params.api_key,
/// api_key: os.environ/AZURE_AI_API_KEY
/// "X-APORIA-API-KEY": self.aporia_api_key,
/// api_key=LITELLM_INTERNAL_JOBS_SERVICE_ACCOUNT_NAME,
/// ```
///
/// The existing PascalCase clause already accepts the trade this makes — a
/// credential that is pure letters would survive — and takes it for ONE shape
/// (`password: String`). Two conditions keep it narrow where that one is
/// narrow by accident:
///
///   * a SEPARATOR (`_`, `.`, `/`) must be present, so a passphrase like
///     `correcthorsebatterystaple` is still masked;
///   * NO DIGIT anywhere, so every prefixed vendor token (`sk_live_51H8…`),
///     every base64 blob and every hex string is still masked.
///
/// Each segment must also open on a letter or `_`, so `10.0.0.1` is not an
/// identifier path.
///
/// The veto belongs to the KEYWORD pass only. The provider table matches a
/// KNOWN vendor prefix, and `hf_…`, `npm_…` and a Sentry token are all
/// letters carrying `_` as a prefix separator, so sharing the predicate
/// there unmasked them. One provider row is generic rather than prefixed
/// (`password=` / `pwd=`) and it keeps masking an identifier value: measured
/// 2026-08-28 over 82 692 third-party source files that costs 13 lines in 11
/// files, which does not pay for turning a 98-row `(regex, replacement)`
/// table into per-row opt-in.
/// A port number followed by a URL path: `4000/utils/supported_openai_params`.
///
/// Only ever consulted for a BARE `:` connector inside a string that already
/// carries a `://`, where the colon the regex matched is a URL's port
/// separator rather than an assignment. Rejects the base64 padding and `+`
/// that no URL path carries.
pub(super) fn value_is_port_then_path(value: &str) -> bool {
    let digits = value.chars().take_while(|c| c.is_ascii_digit()).count();
    if !(1..=5).contains(&digits) || value.as_bytes().get(digits) != Some(&b'/') {
        return false;
    }
    !value.contains('+') && !value.contains('=')
}

pub(super) fn value_is_identifier_path(value: &str) -> bool {
    let v = value.trim_end_matches([',', ';', ')', '}', '"', '\'']);
    if v.is_empty() || v.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    if !v.contains('_') && !v.contains('.') && !v.contains('/') {
        return false;
    }
    // `-` is ordinary inside a path segment — `Conf-files`, `trig-aWait`, a
    // kebab-cased module — and refusing it masked `dpkg -l`'s own legend line,
    // `Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/
    // Trig-pend`, which is what tells a reader how to parse every row beneath
    // it. The no-DIGIT requirement above is what keeps this safe: a credential
    // of 32 characters or more with not one digit in it is not a shape this
    // pass needs to cover. Measured 2026-09-02 over one machine's real tool
    // output: 25 values of 3 099 entropy candidates are newly spared.
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
    {
        return false;
    }
    v.split(['.', '/']).all(|seg| {
        seg.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    })
}

/// A FILE PATH being named, not a credential.
///
/// [`value_is_identifier_path`] already spares `self.api_key` and
/// `os.environ/AZURE_AI_API_KEY`, but it refuses any value carrying a DIGIT —
/// and a digit inside a path segment is ordinary: `e2e`, `v2`, `s3`, `oauth2`,
/// `glm-5-2`, `minicpm5-1b`. Measured 2026-08-28 over 53 989 real agent
/// command outputs, the controlled pair is exact:
/// `read: crates/tokenade-cli/tests/e2e/wiring.rs` is masked and
/// `read: crates/tokenade-cli/tests/ee/wiring.rs` is not — one digit, in a
/// directory name, decides. Fifteen of the sixty-seven masked lines in that
/// corpus are the HARNESS's own notice, `[This command modified 1 file you've
/// previously read: <path>]`, served to the agent as
/// `previously read: <redacted>.rs` — told a file changed, not which.
///
/// `after` is the text FOLLOWING the captured value, and it is needed rather
/// than decorative: the entropy pass's value class is
/// `[A-Za-z0-9+/_=\-]{32,}`, which has no `.`, so the extension always falls
/// outside the match. `…/tests/e2e/wiring` is what the veto is handed; `.rs`
/// is what says it is a file.
///
/// The extension is the discriminator, and it is chosen because base64 cannot
/// produce it: `.` is not in the base64 alphabet, so a credential blob is
/// never followed by `.<short alnum>` — while a token that carries no slash at
/// all (`xoxb-…`, `sk-proj-…`) never reaches this test.
pub(super) fn value_is_file_path(value: &str, after: &str) -> bool {
    let v = value.trim_end_matches([',', ';', ')', '}', '"', '\'']);
    if v.is_empty() || v.contains("://") || !v.contains('/') {
        return false;
    }
    if !v.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-' | '+' | '@' | '~' | '=')
    }) {
        return false;
    }
    // `=` is admitted ONLY inside a segment, alphanumeric on both sides. That
    // is Hive partition syntax — `data/year=2026/month=08/day=15/part-00042-…`
    // — the layout Spark, Glue and Athena write by default, so it is what a
    // recursive object listing of a partitioned bucket is made of. Every one
    // of those keys carries a `=`, so refusing the character did not spare
    // some lines of such a listing, it masked ALL of them: measured on a
    // 1500-key `aws s3 ls --recursive` sample, 1500 lines of 1500.
    //
    // base64 padding is the opposite shape and stays refused: `=` there is
    // always TRAILING, never flanked.
    //
    // The measured effect on THIS machine's transcripts is zero, and that is
    // stated rather than hidden: of 244 entropy candidates carrying a relative
    // path, 24 were refused on the `=` and only 2 of those are flanked — and
    // both of those two are still masked, correctly, for having no extension
    // after them. Nothing here reads S3. The evidence for this veto is the
    // SHAPE, which is what every partitioned bucket in the world is made of,
    // not a local count.
    let b = v.as_bytes();
    if b.iter().enumerate().any(|(i, &c)| {
        c == b'='
            && !(i > 0
                && i + 1 < b.len()
                && b[i - 1].is_ascii_alphanumeric()
                && b[i + 1].is_ascii_alphanumeric())
    }) {
        return false;
    }
    // A trailing `/` names a directory, and there is no basename to extend.
    if v.rsplit('/').next().is_none_or(str::is_empty) {
        return false;
    }
    let ext: String = after
        .strip_prefix('.')
        .unwrap_or("")
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    // No boundary test after this: `take_while` already stopped at the first
    // non-alphanumeric byte, so one here can never fail. It was written, it
    // survived its own mutation green, and it is gone.
    (1..=6).contains(&ext.len())
}

pub(super) fn apply_provider_patterns(s: &str) -> String {
    let ps = patterns();
    let mut out = std::borrow::Cow::Borrowed(s);
    for i in 0..ps.raw.len() {
        let (re, replace_with) = ps.rule(i);
        // `is_match` before `replace_all`: on a non-matching haystack
        // `replace_all(..).into_owned()` still copies the whole string, and
        // almost every rule misses almost every time.
        if re.is_match(&out) {
            let replaced = re
                .replace_all(&out, |c: &Captures| {
                    let whole = c.get(0).expect("group 0 always exists");
                    // Group 1, where a rule has one, is the prefix the
                    // replacement keeps (`${1}` — the scheme or the key name).
                    // What follows it is the value about to be masked.
                    let cut = c.get(1).map(|m| m.end() - whole.start()).unwrap_or(0);
                    let value = &whole.as_str()[cut..];
                    // NOT `unquoted(value)`, unlike the keyword pass.
                    // Measured before sharing it: every provider row whose
                    // value class could carry a quote excludes `"` and `'`
                    // from that class, so a quoted placeholder never reaches
                    // this closure at all — `GITHUB_TOKEN="{env:GH_TOKEN}"`
                    // is already untouched here. A veto that cannot fire is
                    // not symmetry, it is an unpinnable line.
                    if value_is_placeholder(value)
                        || value_is_working_directory(&whole.as_str()[..cut], value)
                    {
                        return whole.as_str().to_string();
                    }
                    let mut dst = String::new();
                    c.expand(replace_with, &mut dst);
                    dst
                })
                .into_owned();
            out = std::borrow::Cow::Owned(replaced);
        }
    }
    out.into_owned()
}

pub(super) fn patterns() -> &'static PatternSet {
    static P: OnceLock<PatternSet> = OnceLock::new();
    P.get_or_init(|| {
        let _p = crate::profile::phase("redact:pattern_table");
        let rules: Vec<(&str, &str)> = rules::RULES.to_vec();
        let compiled = rules.iter().map(|_| OnceLock::new()).collect();
        PatternSet {
            raw: rules,
            compiled,
        }
    })
}
