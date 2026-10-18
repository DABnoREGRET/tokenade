//! `SECRET = value` in any of the shapes a config or a shell writes it.
//!
//! Gated on a cheap pre-filter: the expensive rules only run on input that
//! holds an assignment at all.

use super::*;

/// True when an opt-out env var is set to a non-empty, non-"0" value.
/// The entropy/PAN passes are high false-positive risk on this hot path, so
/// each has an escape hatch (`TOKENADE_NO_REDACT_ENTROPY`,
/// `TOKENADE_NO_REDACT_PAN`). PII (email/phone/IBAN/SSN) is different: an email
/// is not an auth secret and masking it degrades every `git log`/`Author:`/log
/// line on every output, so it is OFF BY DEFAULT and must be opted IN with
/// `TOKENADE_REDACT_PII=1`. Read per-call (getenv is cheap) so tests and
/// long-lived hooks can toggle it.
pub(super) fn opt_out(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

/// Opt-IN gate (default OFF): true only when the env var is set to a truthy
/// value. Used for PII, whose hot-path utility cost isn't worth a default-on.
pub(super) fn opt_in(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

/// A value that is CODE, not a credential.
///
/// The keyword rules below match `<keyword><sep><value>` with a deliberately
/// permissive value class, because a credential can be almost any run of
/// non-space bytes. That same permissiveness makes them match ordinary source:
/// `let user_password = form.get("pw");` and `let secret = derive(seed);` had
/// their right-hand sides masked, and a page of Django docs came back saying
/// `primary_key=<redacted>`. An agent reads code all day and then EDITS against
/// what it read, so a silently corrupted line is worse than a verbose one.
///
/// The signal is narrow on purpose: a call `(`/`)` or a path separator `::`.
/// No credential format in existence contains either — not base64, not hex, not
/// JWT, not any provider's prefixed token. So this gives up nothing real.
pub(super) fn value_is_code(value: &str) -> bool {
    // The quotes the rule captured are not part of the value. Stripped once,
    // here, so every clause below sees the same thing whether the source wrote
    // `api_key: null` or `"api_key": "null"`.
    let value = super::unquoted(value);
    // A call, a path, or a generic. None of these appear in any credential
    // format — not base64, not hex, not JWT, not a provider-prefixed token.
    if value.contains('(') || value.contains(')') || value.contains("::") {
        return true;
    }
    // A TYPE with parameters or a subscript, by the same argument and for the
    // same reason the PascalCase clause below exists — it only covered a BARE
    // word, so `password: String` was safe while `api_key: Optional[str]` was
    // not. Measured 2026-08-28 over 1 993 third-party source files: 267 carried
    // a redaction, and `litellm/__init__.py` line 268 reached the agent as
    // `api_key: <redacted> = None` where the file says
    // `api_key: Optional[str] = None`. The type is the one thing that line is
    // FOR. Same for `Vec<String>`, `Option<String>`, `Dict[str, Any]`.
    //
    // No credential format carries any of these four bytes: base64 is
    // `A-Za-z0-9+/=`, base64url swaps in `-_`, hex is hex, and every provider
    // prefix is alphanumeric with `-`/`_`.
    //
    // On the OPENING bracket only. Keyed on `]`/`>` as well, this veto was
    // defeated by the bracket that CLOSES the text around the match:
    // `[tokenade: redirected — … ?token=sk-live-…]` gives the value class
    // `sk-live-…]`, which carried a `]` and walked straight through. A real
    // credential, echoed into the agent's context — caught by
    // `provenance_does_not_leak_credentials_from_the_landing_url`, which is why
    // that test exists. A type OPENS its bracket inside the value; a credential
    // bumping into a closer does not.
    if value.contains('[') || value.contains('<') {
        return true;
    }
    // A variable reference or a fill-me-in marker — `$VAR`, `{var}`,
    // `<your key>`, `%s`. Same predicate the provider pass uses, because the
    // two rule families see the same lines: with the veto in one pass only,
    // `export OPENAI_API_KEY={master_key}` simply moved from one tally to the
    // other and the agent still read `<redacted>`.
    if super::value_is_placeholder(value) {
        return true;
    }
    // An identifier or a dotted path being passed through — `self.api_key`,
    // `smtp_password`, `os.environ/AZURE_AI_API_KEY`. Same predicate the
    // provider pass uses (a veto in one pass only just moves the
    // tally).
    if super::value_is_identifier_path(value) {
        return true;
    }
    // The match ate half of a COMPARISON. `api_key == "x"` is
    // `api[_\-]?key\s*=\s*` against the FIRST `=`, leaving the second one as
    // the value: the agent was served `if api_key =<redacted> "x":`, which is
    // not merely masked, it is syntactically wrong — and an agent edits against
    // what it read. `!=`, `>=`, `<=` and `=>` all land here too.
    //
    // A credential never begins with `=`: base64 padding is trailing only.
    if value.starts_with('=') {
        return true;
    }
    // A PRIMITIVE TYPE or a language literal, spelled lowercase. The clause
    // below only recognises a BARE PascalCase word, which is how Rust, Go, Java
    // and C# spell a type — and not how Python, TypeScript or Ruby do. Measured
    // 2026-08-28 across 1 993 third-party source files: `password: str | None =
    // None` and `api_key: str | None = None` reached the agent with the type
    // masked, and Python's modern optional syntax puts that shape in every
    // settings class there is.
    //
    // An exact list, not a heuristic: each of these is a word a language
    // reserves, so no credential can be one without being a keyword collision
    // its own issuer would have to have chosen deliberately.
    const CODE_WORDS: &[&str] = &[
        "str",
        "int",
        "bool",
        "float",
        "bytes",
        "list",
        "dict",
        "set",
        "tuple",
        "string",
        "number",
        "boolean",
        "object",
        "any",
        "unknown",
        "never",
        "void",
        "null",
        "nil",
        "none",
        "undefined",
        "true",
        "false",
        "self",
        "this",
        "optional",
    ];
    let bare = value.trim_end_matches([';', ',', ')']);
    if CODE_WORDS.contains(&bare.to_ascii_lowercase().as_str()) {
        return true;
    }
    // A bare PascalCase word with no digits: `password: String`,
    // `secret: Vec`, `token: Option`. This is a type annotation in every
    // curly-brace language, and it was being masked in struct definitions.
    // The trade is explicit: a literal password that is pure letters AND
    // starts with a capital AND has no digit would now survive. Every
    // credential worth the name has entropy this shape cannot carry.
    let v = value.trim_end_matches([';', ',']);
    !v.is_empty()
        && v.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && v.chars().all(|c| c.is_ascii_alphabetic())
}

/// The value is the KEY spelled again — `api_key=api_key`, `password: password`.
///
/// This is an argument being passed through, and it is what
/// `openai.OpenAI(api_key=api_key, base_url=api_base)` looks like: the value
/// class runs to the next space, so it swallowed `api_key,` and the agent read
/// `OpenAI(api_key=<redacted> base_url=api_base)` — a call it could no longer
/// reproduce. The `(`/`)` veto cannot see it, because the parentheses are
/// around the match, not inside the value.
///
/// No credential is its own field name.
pub(super) fn value_echoes_its_key(pre: &str, value: &str) -> bool {
    let key: String = pre
        .trim_end_matches([':', '=', ' ', '\t', '"'])
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if key.len() < 3 {
        return false;
    }
    let v = value.trim_end_matches([';', ',', ')']);
    !v.is_empty() && v.eq_ignore_ascii_case(&key)
}

/// The keyword-assignment rules, applied with [`value_is_code`] as a veto.
///
/// Kept out of `patterns()` because a plain (regex, replacement) pair cannot
/// look at what it is about to mask.
/// A test-runner STATUS line is not a credential assignment.
///
/// `PASS` lowercases to `pass`, which this module's rule table carries as a
/// keyword in its own right (beside `password`/`passwd`/`pwd`), so
/// `--- PASS: TestSum000` reads as a field named `pass` holding a value. Every
/// PASSING test then loses its name on `go test -v`, the most common Go
/// invocation there is, while `--- FAIL:` keeps its own (no `pass` substring) —
/// the successes were masked and the failures were not, which is the harder
/// half to notice.
///
/// The module already carries this finding for Rust: `cargo test` prints
/// `web::tests::a11y_preserves_refs…` and the `::` veto exists so the token
/// naming WHICH test ran survives. Go writes its status with `---`, so it needs
/// its own reading.
///
/// Anchored on the `---` prefix rather than on the word `pass` alone: a bare
/// `pass: hunter2` in a YAML config IS a credential and must keep masking.
/// `secret: the recipe uses butter` is prose with a colon, not a field: the
/// "value" is the first word of a sentence that goes on. graphify fixed the
/// same false positive (a bare "tokens" in prose). Narrow on every side: the
/// `key: value` form only, a value of lowercase letters only (no digit, no
/// case mix — every generated credential has one), and at least two more
/// lowercase words after it on the line. `password: hunter2` and a
/// `secret: s3cr3t` alone on its line are still masked.
fn value_opens_a_sentence(pre: &str, value: &str, after: &str) -> bool {
    if !pre.trim_end().ends_with(':')
        || value.is_empty()
        || !value.bytes().all(|b| b.is_ascii_lowercase())
    {
        return false;
    }
    let mut words = after.split(' ');
    words.next() == Some("")
        && words
            .take(2)
            .map(|w| w.trim_end_matches([',', '.', ';', ':', '!', '?', ')']))
            .filter(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()))
            .count()
            == 2
}

/// Nothing writes a secret after `--- PASS:`.
fn line_is_test_runner_status(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("---") else {
        return false;
    };
    let rest = rest.trim_start().as_bytes();
    // Compared as BYTES. The keywords are ASCII, so the first `k.len()` bytes
    // are exactly the test — while `rest[..k.len()]` on a `&str` PANICS when
    // that offset lands inside a multibyte character. MEASURED on 1.1.9: a
    // 160 KB log carrying one `--- aéé pass=…` line killed `tokenade read`
    // AND `tokenade wrap` with "end byte index 4 is not a char boundary; it
    // is inside 'é'", losing the whole output.
    ["PASS", "FAIL", "SKIP", "BENCH"]
        .iter()
        .any(|k| rest.len() >= k.len() && rest[..k.len()].eq_ignore_ascii_case(k.as_bytes()))
}

/// Byte ranges of the lines that are test-runner status lines.
///
/// Computed ONCE per haystack. It used to be answered per MATCH, by scanning
/// backwards from the match for a newline and then building a string of
/// everything between: on text that is one long line — a `key=value` log with
/// no newlines is exactly that — the scan and the string it allocated were
/// both the size of the input, once for every match. MEASURED on 1.1.9:
/// `rd:keyword` took 329 ms on 507 KB and 4 901 ms on 2.03 MB, four times the
/// bytes for fifteen times the work, and `tokenade read` on a 4 MB log spent
/// 20 s in it. Nearly every text has no such line at all, so this is usually
/// an empty vector and the per-match answer is a comparison against nothing.
fn status_line_ranges(hay: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for line in hay.split_inclusive('\n') {
        let end = start + line.len();
        if line_is_test_runner_status(line) {
            ranges.push((start, end));
        }
        start = end;
    }
    ranges
}

/// Whether the match at `at` sits on one of those lines.
fn is_test_runner_status(ranges: &[(usize, usize)], at: usize) -> bool {
    ranges
        .binary_search_by(|&(s, e)| {
            if at < s {
                std::cmp::Ordering::Greater
            } else if at >= e {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

pub(super) fn keyword_assignment_rules() -> &'static [(Regex, &'static str)] {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    RULES.get_or_init(|| {
        let _p = crate::profile::phase("redact:compile:keyword_rules");
        [
            r#"(?i)(api[_\-]?key\s*=\s*)(?:'[^']*'|"[^"]*"|[^\s"'`&]+)"#,
            r#"(?i)("?\b(?:password|passwd|pwd|pass|secret|api[_\-]?key|access[_\-]?token|auth[_\-]?token)"?\s*:\s+)(?:'[^']*'|"[^"]*"|[^\s,}}"'`]+)"#,
            r#"(?i)(\b_auth(?:token)?\s*=\s*)(?:'[^']*'|"[^"]*"|[^\s"'`]+)"#,
            r#"(?i)(_password\s*=\s*)(?:'[^']*'|"[^"]*"|[^\s"'`]+)"#,
            // Split in two on 2026-08-03, from a field report that cost a real
            // fix: this used to be ONE rule allowing spaces around `=` with a
            // bare value class. Reading Rust source, `let token = line.split(':')`
            // came back as `token = <redacted> ':').nth(1)…`, and the agent then
            // patched a line it had never seen whole — its `re.subn` reported
            // zero replacements and the clippy fix was silently never applied.
            // Same class as the SCREAMING_SNAKE rule further down, same lesson:
            // agents read code all day and then EDIT against what they read, so
            // a silently corrupted line is worse than a verbose one.
            //
            // The discriminator is spacing, and it is the one the shapes
            // themselves provide: configuration, env files and CLI flags write
            // `token=abc` with no spaces; code writes `token = expr` with them.
            //
            // (a) QUOTED value, spaces allowed — a hardcoded literal after one
            // of these names is worth masking wherever it appears.
            r#"(?i)(\b(?:PGPASSWORD|MYSQL_PWD|REDISCLI_AUTH|password|passwd|pwd|pass|secret|token|access[_\-]?token|auth[_\-]?token)\s*=\s*)(?:'[^']*'|"[^"]*")"#,
            // (b) BARE value, and only in the no-space shape. An expression is
            // never written this way, so `let token = foo.bar()` is left alone
            // while `PGPASSWORD=hunter2` is not.
            r#"(?i)(\b(?:PGPASSWORD|MYSQL_PWD|REDISCLI_AUTH|password|passwd|pwd|pass|secret|token|access[_\-]?token|auth[_\-]?token)=)(?:[^\s"'`&;]+)"#,
            // Keyword then WHITESPACE then value — the command-line shape.
            // `aws configure set aws_secret_access_key <VALUE>` carries the
            // keyword right there, but none of the rules above match it
            // because the separator is a space rather than `=` or `:`, so the
            // one half of an AWS credential that actually matters travelled in
            // clear. Same for `--password <VALUE>` and friends.
            //
            // Kept deliberately narrow: only names that are unambiguous on
            // their own (never bare `pass`/`token`, which are ordinary
            // English), and a value of at least 12 characters that is not
            // itself a flag — so "set aws_secret_access_key to your key"
            // stays readable prose. `<PLACEHOLDER>` and `$SHELL_VAR` are
            // excluded too: documentation and shell references are not
            // secrets, and masking them makes a command unreadable for no
            // gain.
            r#"(?i)((?:\b(?:aws_secret_access_key|aws_session_token)|--(?:password|passwd|token|api[-_]?key|secret|access[-_]token|auth[-_]token|client[-_]secret))\s+)(?:'[^']*'|"[^"]*"|[^\s"'`\-<$][^\s"'`]{11,})"#,
        ]
        .into_iter()
        .map(|r: &str| (Regex::new(r).unwrap(), "<redacted>"))
        .collect()
    })
}

/// True when at least one literal is present — the only case where compiling
/// and running the keyword rules can change anything.
pub(super) fn may_hold_keyword_assignment(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    KEYWORD_LITERALS.iter().any(|k| lower.contains(k))
}

pub(super) fn redact_keyword_assignments(s: &str) -> String {
    // Measured 2026-08-04 in RELEASE: compiling these rules costs ~2,2 ms and
    // `wrap` totals ~6 ms, so a third of every wrapped command went to rules
    // that cannot fire on `echo hi`. Checking six substrings first costs
    // microseconds and, on the common case, avoids the compile entirely.
    if !may_hold_keyword_assignment(s) {
        return s.to_string();
    }
    let mut out = s.to_string();
    for (re, mask) in keyword_assignment_rules() {
        // The haystack is bound so the closure can look at what PRECEDES the
        // match: the rules capture from the keyword (`\b`), so a Go status line
        // gives `pre = "PASS: "` with the `---` sitting outside the capture.
        let src_line = std::mem::take(&mut out);
        let status_lines = status_line_ranges(&src_line);
        out = re
            .replace_all(&src_line, |c: &Captures| {
                let m0 = c.get(0).expect("group 0 always matches");
                let whole = m0.as_str();
                let pre = c.get(1).map(|m| m.as_str()).unwrap_or_default();
                let value = &whole[pre.len()..];
                // `PWD=` after a `;` belongs to a connection string: the
                // key alone does not say working directory there.
                let after_semicolon = src_line[..m0.start()].ends_with(';');
                if value_is_code(value)
                    || value_opens_a_sentence(pre, value, &src_line[m0.end()..])
                    // `« secret: » n'est plus masquée`: a value with no letter
                    // and no digit (`»`, `—`, `""`) holds no secret to hide.
                    || !value.chars().any(char::is_alphanumeric)
                    || value_echoes_its_key(pre, value)
                    || (!after_semicolon && value_is_working_directory(pre, value))
                    || is_test_runner_status(&status_lines, m0.start())
                {
                    whole.to_string()
                } else {
                    format!("{pre}{mask}")
                }
            })
            .into_owned();
    }
    out
}

#[cfg(test)]
mod code_veto_tests {
    use super::super::redact;

    /// The keyword rules match `<keyword><sep><value>` with a value class
    /// permissive enough to catch any credential, and that same permissiveness
    /// eats ordinary source. Measured 2026-08-28 over 1 993 third-party files:
    /// 267 carried at least one redaction, and these are the shapes behind it.
    ///
    /// An agent reads code all day and then EDITS against what it read, so a
    /// silently corrupted line is worse than a verbose one.
    /// `--- PASS: TestX` is a test-runner status line, not a credential field.
    ///
    /// `PASS` carries the substring `pass`, which this table treats as a
    /// keyword, so every PASSING Go test lost its name on `go test -v` while
    /// `--- FAIL:` kept its own. The pair below is the whole point: the status
    /// line survives, and a `pass:` field with no `---` in front still masks.
    /// One long line must not cost the input size TIMES the number of matches.
    ///
    /// A `key=value` log with no newlines is one line with a match every few
    /// bytes. The veto used to scan backwards from each match and build a
    /// string of everything before it. MEASURED on 1.1.9, `rd:keyword` alone:
    /// 329 ms on 507 KB and 4 901 ms on 2.03 MB — four times the bytes, fifteen
    /// times the work — and `tokenade read` on a 4 MB log took 20 s.
    ///
    /// The deadline is loose on purpose. It is not a benchmark: it fails only
    /// for work that grows faster than the input, which at this size is
    /// seconds even before the debug profile multiplies it.
    #[test]
    fn one_long_line_of_assignments_stays_linear() {
        let line = "key=value token=abc123def456 id=42 ".repeat(90_000);
        assert!(
            !line.contains('\n'),
            "the point of the fixture is that it is ONE line"
        );
        let started = std::time::Instant::now();
        let out = super::redact_keyword_assignments(&line);
        let took = started.elapsed();
        assert!(
            out.len() < line.len(),
            "nothing was masked, so this measured the wrong thing"
        );
        assert!(
            took < std::time::Duration::from_secs(5),
            "{} bytes on one line took {took:?} — the veto is scanning per match again",
            line.len()
        );
    }

    /// A `---` line whose text is not ASCII must not kill the process.
    ///
    /// The keyword check compared `rest[..k.len()]` on a `&str`, and that
    /// offset can land inside a multibyte character. MEASURED on 1.1.9: a
    /// 160 KB log carrying one `--- aéé pass=…` line killed BOTH
    /// `tokenade read` and `tokenade wrap` with "end byte index 4 is not a
    /// char boundary; it is inside 'é'" — the whole output lost, and the
    /// message naming a byte offset rather than a line.
    #[test]
    fn an_accented_dash_line_does_not_kill_the_run() {
        let kw = format!("{}{}", "pa", "ss");
        // Byte 4 of the text after `---` falls inside the second `é` here,
        // which is what the old slice indexed into. The neighbours on either
        // side are boundary-safe, so a fixture one character off proves
        // nothing — they are kept to show the difference is the OFFSET.
        for text in ["aéé", "aaéé", "éé", "ééé", "aaaéé"] {
            let line = format!("--- {text} {kw}=abc123def456");
            let out = super::super::redact(&line);
            assert!(out.contains(text), "the line's own text was lost: {out}");
            assert!(
                !out.contains("abc123def456"),
                "this is not a status line, so the value must still be masked: {out}"
            );
        }
    }

    /// The status-line veto is answered from the LINE, not by scanning back
    /// from every match — see `status_line_ranges`. These pin that the answer
    /// is unchanged, including for a match far along a very long line.
    #[test]
    fn the_veto_still_reads_the_line_it_is_on() {
        let kw = format!("{}{}", "PA", "SS");
        let name = format!("{}{}", "TestSum", "000");
        // A status line, with a keyword assignment much later on it.
        let long = format!(
            "--- {kw}: {name} {}token=abc123def456",
            "filler ".repeat(400)
        );
        let out = super::super::redact(&long);
        assert!(out.contains(&name), "status line lost its test name: {out}");

        // The SAME assignment on a line that is not a status line is masked,
        // however long the line is.
        let long2 = format!("start {}token=abc123def456", "filler ".repeat(400));
        let out2 = super::super::redact(&long2);
        assert!(
            !out2.contains("abc123def456"),
            "a value off a status line must still be masked: {out2}"
        );

        // DEEP into the text, so an offset that drifts by one per line has
        // drifted far enough to miss. Three lines cannot show this: the range
        // would still overlap the match.
        let deep = format!(
            "{}--- {kw}: {name}\nmore noise\n",
            "noise line here\n".repeat(50)
        );
        assert!(
            super::super::redact(&deep).contains(&name),
            "a status line 50 lines in was not recognised — line offsets drift"
        );

        // A match starting on the FIRST byte after a status line ends is NOT
        // on that line. Only an inclusive range end gets this wrong.
        let after = format!("--- {kw}: {name}\ntoken=abc123def456\n");
        let out_after = super::super::redact(&after);
        assert!(
            !out_after.contains("abc123def456"),
            "the line AFTER a status line is not the status line: {out_after}"
        );

        // A status line that is not the FIRST line is still found.
        let multi = format!("noise here\n--- {kw}: {name}\nmore noise\n");
        assert!(
            super::super::redact(&multi).contains(&name),
            "a status line after the first was not recognised"
        );
    }

    #[test]
    fn a_test_runner_status_line_keeps_its_test_name() {
        let kw = format!("{}{}", "PA", "SS");
        let name = format!("{}{}", "TestSum", "000");
        for line in [
            format!("--- {kw}: {name} (0.00s)"),
            format!("    --- {kw}: {name}"),
            format!("--- FAIL: {name} (0.00s)"),
            format!("--- SKIP: {name}"),
        ] {
            assert!(
                redact(&line).contains(&name),
                "a test-runner status line lost its test name: {line}"
            );
        }
        // The security half. The veto is anchored on `---`, so the credential
        // shapes it must never touch are named here rather than assumed.
        for (line, secret) in [
            (
                format!("{}: hunter2xyz9", "pa".to_string() + "ss"),
                "hunter2xyz9",
            ),
            (
                format!("{}: hunter2abc123", "pass".to_string() + "word"),
                "hunter2abc123",
            ),
            (
                "api_key: AKIAIOSFODNN7EXAMPLE".to_string(),
                "AKIAIOSFODNN7EXAMPLE",
            ),
            // Same keyword, no `---`: still a field, still masked.
            (format!("{kw}: {name}"), name.as_str()),
        ] {
            assert!(
                !redact(&line).contains(secret),
                "a credential survived the test-status veto: {line}"
            );
        }
    }

    #[test]
    fn ordinary_source_keeps_its_meaning() {
        for (src, must_keep) in [
            // A type annotation is the one thing these lines are FOR.
            ("api_key: Optional[str] = None", "Optional[str]"),
            ("password: str | None = None", "str"),
            ("api_key: str | None = None", "str"),
            ("token: Vec<String> = vec![]", "Vec<String>"),
            ("secret: Dict[str, Any] = {}", "Dict[str"),
            // A pass-through argument, not a literal.
            (
                "client = openai.OpenAI(api_key=api_key, base_url=api_base)",
                "api_key=api_key",
            ),
            ("password: password", "password: password"),
        ] {
            let out = redact(src);
            assert!(
                out.contains(must_keep),
                "redacted ordinary source: {src:?} -> {out:?}"
            );
        }
    }

    /// A COMPARISON is not an assignment. `api_key == "x"` matched on the first
    /// `=` and masked the second, so the agent was served `api_key =<redacted>
    /// "x"` — not merely masked, syntactically wrong.
    #[test]
    fn a_comparison_is_not_corrupted() {
        for src in [
            "if api_key == \"invalid-api-key\":",
            "if api_key != other_key:",
            "assert api_key == expected",
        ] {
            let out = redact(src);
            assert!(
                !out.contains("=<redacted>"),
                "half a comparison operator survived: {src:?} -> {out:?}"
            );
        }
    }

    /// Prose with a colon is not a field: the first word of a sentence was
    /// masked (`secret: <redacted> recipe uses butter`). Its twins that ARE
    /// credentials keep masking: a digit, a capital, or nothing after.
    #[test]
    fn a_sentence_after_a_colon_is_not_a_credential() {
        for src in [
            "secret: the recipe uses butter",
            "password: see the settings page",
            "## piste 1 pass: two anolisa ideas, both clean negatives",
            "une phrase après « secret: » n'est plus masquée",
            "password: \u{2014}",
        ] {
            assert_eq!(redact(src), src);
        }
        for src in [
            "password: hunter2 was rejected today",
            "secret: xk9pqz goes in the vault",
            "password: letmein",
        ] {
            assert!(redact(src).contains("redacted"), "{src}");
        }
    }

    /// And the whole point still holds: a real credential is still masked.
    /// Every veto above is a hole if this stops being true.
    #[test]
    fn a_real_credential_is_still_masked() {
        for src in [
            "api_key = \"sk-proj-Ab3dEf6hIj9lMn2pQr5tUv8xYz1a\"",
            "api_key=sk-proj-Ab3dEf6hIj9lMn2pQr5tUv8xYz1a",
            "password: hunter2trombonestaple",
            "access_token: ghp_Ab3dEf6hIj9lMn2pQr5tUv8xYz1aB2c3",
            "api_key: \"AKIA7QZ3MPLW2XKD9RTV\"",
            // The bracket that CLOSES the text around the match is not a type.
            // Keying the veto on `]`/`>` as well as `[`/`<` let this exact
            // shape through — a real credential in a provenance note.
            "[tokenade: redirected — this is https://cdn.test/x?token=sk-live-Ab3dEf6hIj9lMn2p]",
            "{\"api_key\": \"sk-proj-Ab3dEf6hIj9lMn2pQr5tUv8xYz1a\"}",
        ] {
            let out = redact(src);
            assert!(
                out.contains("redacted"),
                "a credential went through unmasked: {src:?} -> {out:?}"
            );
        }
    }
}
