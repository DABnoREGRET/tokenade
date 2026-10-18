//! **What has the SHAPE of a secret without carrying its name.**
//!
//! The last net: a string long enough, disordered enough, and that is
//! neither a UUID nor a known hash. It is the most dangerous rule of the module
//! — it relies on no keyword, so it can fire anywhere, and
//! its thresholds are measured rather than chosen.

use super::*;

// ---------------------------------------------------------------------------
// J — generic high-entropy secret sweep. Runs LAST and behind the strictest
// gate (`TOKENADE_NO_REDACT_ENTROPY`) because it is the highest FP risk.
// A value is redacted ONLY when it is a ≥32-char token appearing after `=`/`:`
// (or a secret keyword) AND is high-entropy AND mixes ≥3 character classes —
// which structurally EXCLUDES: git SHAs / hex hashes (2 classes), dictionary
// words (1 class), UUIDs / SRI hashes / data: URIs (explicit skips below).
// ---------------------------------------------------------------------------
pub(super) fn shannon_per_char(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    let mut n = 0u32;
    for b in s.bytes() {
        counts[b as usize] += 1;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    let nf = n as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / nf;
            -p * p.log2()
        })
        .sum()
}

/// Bitmask of present character classes: lower / upper / digit / symbol.
pub(super) fn class_count(s: &str) -> u32 {
    let mut m = 0u8;
    for b in s.bytes() {
        m |= match b {
            b'a'..=b'z' => 1,
            b'A'..=b'Z' => 2,
            b'0'..=b'9' => 4,
            _ => 8,
        };
    }
    m.count_ones()
}

pub(super) fn is_uuid(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
            .unwrap()
    })
    .is_match(s)
}

pub(super) fn entropy_is_secret(val: &str, ctx_before: &str) -> bool {
    // Sub-resource-integrity / hash prefixes are legitimate, keep them.
    let lower = val.to_ascii_lowercase();
    if lower.starts_with("sha1-")
        || lower.starts_with("sha256-")
        || lower.starts_with("sha384-")
        || lower.starts_with("sha512-")
        || lower.starts_with("md5-")
    {
        return false;
    }
    // Embedded image / data URI base64 — not a credential.
    if ctx_before.contains("data:") || ctx_before.ends_with("base64,") {
        return false;
    }
    if is_uuid(val) {
        return false;
    }
    // <3 classes ⇒ pure hex (git SHA / hash = 2) or a dictionary word (1).
    if class_count(val) < 3 {
        return false;
    }
    if is_name_around_a_hash(val) {
        return false;
    }
    shannon_per_char(val) >= 3.5
}

/// Quoted-printable TEXT: `=XX` byte escapes INSIDE the value. An encoded
/// e-mail subject (`=?utf-8?q?R=C3=A9union_budget_Q3_=E2=80=94_d=C3=A9cision?=`)
/// matched `R=` as an assignment, and the rest — escapes, `_` for spaces,
/// three classes — cleared the entropy floor: the subject came back
/// `<redacted>`. Base64 only ever puts `=` at the END (padding), so two
/// escapes before the end cannot be a base64 secret; a hex escape is two
/// UPPERCASE hex digits, which a random alphabet would rarely line up twice.
fn is_quoted_printable(val: &str) -> bool {
    let b = val.trim_end_matches('=').as_bytes();
    let escapes = (0..b.len().saturating_sub(2))
        .filter(|&i| {
            b[i] == b'='
                && b[i + 1..i + 3]
                    .iter()
                    .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(c))
        })
        .count();
    escapes >= 2
}

/// A NAME built around a hash: `docker-<64 hex>`, `var-lib-docker-overlay2-
/// <64 hex>-merged` (systemd's escaped mount path). A bare hex hash is
/// already spared above (two classes); the word prefix only adds `-`, and
/// with it a third class — measured 2026-10-03 on a real `journalctl -n 400`
/// of this machine: 34 unit names `<redacted>.scope` / `<redacted>.mount`,
/// every Docker container's start and stop. Spared only when every segment
/// is a lowercase word (trailing digits allowed: `overlay2`) or hex, and one
/// hex segment is a hash (≥ 32): a random token's segments are neither.
fn is_name_around_a_hash(val: &str) -> bool {
    let mut hash = false;
    for seg in val.split(['-', '_']) {
        let hex = seg.len() >= 32
            && seg
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let digits_from = seg.find(|c: char| c.is_ascii_digit()).unwrap_or(seg.len());
        let word = digits_from > 0
            && seg[..digits_from].bytes().all(|b| b.is_ascii_lowercase())
            && seg[digits_from..].bytes().all(|b| b.is_ascii_digit());
        hash |= hex;
        if !hex && !word {
            return false;
        }
    }
    hash
}

/// A PATH of words: it has a `/`, and every segment between `/ - _ = .` is a
/// lowercase word, a number, or one character (`S`). A random token has
/// segments that mix case or interleave digits, and a passphrase written
/// as `correct3-horse4-…` mixes words and digits or has no `/`, so both
/// still mask. At least four segments: a short value proves nothing.
fn is_made_of_words(val: &str) -> bool {
    let segs: Vec<&str> = val
        .split(['/', '-', '_', '=', '.'])
        .filter(|s| !s.is_empty())
        .collect();
    val.contains('/')
        && segs.len() >= 4
        && segs.iter().all(|seg| {
            seg.len() == 1
                || seg.bytes().all(|b| b.is_ascii_lowercase())
                || seg.bytes().all(|b| b.is_ascii_digit())
        })
}

/// The last word of the key (snake, kebab or camel case) is `id`, `ids`,
/// `uuid`, `urn`, `segment` or `slug`, and no word says it guards access.
fn key_names_an_identifier(key_tail: &str) -> bool {
    let key = key_tail
        .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .next()
        .unwrap_or("");
    let mut words: Vec<String> = Vec::new();
    for part in key.split(['_', '-']).filter(|p| !p.is_empty()) {
        let mut cur = String::new();
        for c in part.chars() {
            if c.is_ascii_uppercase() && cur.chars().last().is_some_and(|l| l.is_ascii_lowercase())
            {
                words.push(std::mem::take(&mut cur));
            }
            cur.push(c.to_ascii_lowercase());
        }
        words.push(cur);
    }
    const GUARDS: [&str; 9] = [
        "session", "token", "secret", "key", "auth", "password", "cookie", "csrf", "sid",
    ];
    words.last().is_some_and(|w| {
        matches!(
            w.as_str(),
            "id" | "ids" | "uuid" | "urn" | "segment" | "slug"
        )
    }) && !words.iter().any(|w| GUARDS.contains(&w.as_str()))
}

/// `.ext` with 1–5 alphanumerics, then the end of the token.
fn followed_by_an_extension(after: &str) -> bool {
    let Some(rest) = after.strip_prefix('.') else {
        return false;
    };
    let ext = rest
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(rest.len());
    (1..=5).contains(&ext)
        && rest[ext..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || "\"'`<>,;:)]}".contains(c))
}

/// `name=rest` after the spared `name` (a Cargo directive, a JVM option),
/// put back through this pass. `rest` alone has no connector left in front
/// of it, so a bare credential would sail through: `rest` is examined as
/// `name`'s value unless it carries its own `KEY=` (`rustc-env=K=V`).
fn reexamined(name: &str, rest: &str) -> String {
    if rest.contains('=') {
        format!("{name}={}", redact_entropy(rest))
    } else {
        redact_entropy(&format!("{name}={rest}"))
    }
}

pub(super) fn redact_entropy(s: &str) -> String {
    if s.len() < 32 {
        return s.to_string();
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // `pre` = a secret keyword or a bare `=`/`:` connector, plus the
        // opening quote; `val` = the ≥32-char candidate token.
        Regex::new(
            r#"(?i)(?P<pre>(?:key|token|secret|password|passwd|pwd|credential|apikey|api[_\-]?key|auth)[\w.\-]*\s*[:=]\s*['"]?|[:=]\s*['"]?)(?P<val>[A-Za-z0-9+/_=\-]{32,})"#,
        )
        .unwrap()
    });
    re.replace_all(s, |c: &Captures| {
        let whole = c.get(0).unwrap();
        let pre = c.name("pre").map(|m| m.as_str()).unwrap_or("");
        let val = c.name("val").map(|m| m.as_str()).unwrap_or("");
        let start = whole.start();
        let ctx = s.get(start.saturating_sub(12)..start).unwrap_or("");
        // `::` is a path separator (Rust, C++), not an assignment — and the
        // `[:=]` alternative above happily matches its SECOND colon. That is
        // not academic: `cargo test` prints
        // `web::tests::a11y_preserves_refs_for_interaction`, whose last
        // segment is 35 chars, mixes three classes (`_` counts as a symbol and
        // `a11y` supplies the digits) and clears the entropy floor. The one
        // token naming WHICH test failed was being replaced by `<redacted>`,
        // on `cargo` output — a primary compaction target for this product.
        // Nothing assigns a credential with `::`, so excluding it costs no
        // secret coverage: a single `:` (headers, YAML, JSON) still matches.
        if pre.starts_with(':') && ctx.ends_with(':') {
            return whole.as_str().to_string();
        }
        // An absolute PATH after a bare connector is a path, not a credential.
        // `/` is in the candidate class (base64 needs it), and a long dotless
        // path clears every threshold: three classes (`/`, `-`, digits) and
        // entropy well over the floor. Found live — a shell error reading
        // `sh: line 12: /tmp/claude-1000/-home-…/scratchpad/rl: No such file`
        // came back with the path replaced by `<redacted>`, which is the whole
        // content of that message. Same failure as the test names: the one
        // token you needed, gone.
        //
        // Deliberately NOT applied when a secret KEYWORD introduced the value.
        // `aws_secret: /Ab3…` still masks — there the key says what it is, and
        // a base64 blob may legitimately open with `/`. Only the bare `:`/`=`
        // connector, where nothing but the shape is talking, defers to the path
        // reading.
        let bare_connector = pre.starts_with(':') || pre.starts_with('=');
        if bare_connector && val.starts_with('/') && val.matches('/').count() >= 2 {
            return whole.as_str().to_string();
        }
        // Same deference, same reason: after a bare connector, quoted-printable
        // text is text. `password=…` keeps masking whatever shape follows.
        if bare_connector && is_quoted_printable(val) {
            return whole.as_str().to_string();
        }
        // Same reading for a path or slug made of WORDS: `auto:aligned-block/
        // S=/tmp/claude-1000/-home-username--cursor-…` (a row of tokenade's
        // own value ranking) was masked whole. Same bare-connector limit.
        if bare_connector && is_made_of_words(val) {
            return whole.as_str().to_string();
        }
        let before = &s[..start];
        // A URL's PORT is not an assignment. `http://localhost:4000/utils/x`
        // matches the bare `[:=]` alternative on the PORT colon, so `val`
        // becomes the port plus the whole path and the line comes back as
        // `curl -X GET 'http://localhost:<redacted>'`. Measured 2026-08-28 on
        // third-party source: 13 of 83 entropy masks are this, every one of
        // them a documented `curl` example whose endpoint is the only thing
        // the line says. A credential never follows `host:port`, and the
        // `://` requirement is what makes that true: `user:pass@host` has no
        // scheme between it and the colon, and a bare `port:` with no URL
        // around it is not one either.
        // Without a scheme too, when the colon is glued to a HOST: `psql`,
        // `docker` and service logs write `localhost:5432/app_db` bare —
        // measured 2026-10-03, 43 `localhost:<redacted>` delivered over the
        // last 1 500 agent transcripts. The host is `localhost` or a dotted
        // name or address (`db.internal.example`, `10.0.0.5`): a bare word
        // (`port:4000/…`) says nothing, and `user:pass@host` never reaches
        // here, its `@` is not a host character.
        let host = before
            .rsplit(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
            .next()
            .unwrap_or("");
        let is_host = host.eq_ignore_ascii_case("localhost")
            || (host.contains('.') && !host.starts_with('.') && !host.ends_with('.'));
        let glued_to_host = pre == ":" && is_host;
        if pre.starts_with(':')
            && value_is_port_then_path(val)
            && (before.contains("://") || glued_to_host)
        {
            return whole.as_str().to_string();
        }
        // A value immediately FOLLOWED by `(` is a function being called, not a
        // credential being assigned. The candidate class excludes `(`, so the
        // parenthesis is never inside `val` and only the next byte can say so.
        // Measured 2026-08-28 on third-party source:
        // `unified_batch_id=_is_base64_encoded_unified_file_id(job.…)` and
        // `# google_text_moderation_obj = _ENTERPRISE_GoogleTextModeration()`
        // both came back with the callee replaced by `<redacted>` — a call site
        // the agent could no longer reproduce. Nothing calls a credential.
        if s[whole.end()..].starts_with('(') {
            return whole.as_str().to_string();
        }
        // Followed by an EXTENSION, the value is a file name: `== BREATHE_
        // NORTHWIND_dossier-presse_2026-09-07_v3-final.txt` (a multi-file
        // listing header) and `file=Very_Long_Report_…_2026-09-10.pdf` came
        // back as `<redacted>.txt`. `.` is outside the candidate class, so only
        // what follows can say it. A JWT's next segment is far longer than an
        // extension, so a token never ends here.
        if followed_by_an_extension(&s[whole.end()..]) {
            return whole.as_str().to_string();
        }
        // Followed by `: `, the value is itself a KEY: `105:scored_tweets_
        // model_weight_video_playback50: The probability…` (a `grep -n` over
        // a list of named fields) — 97 masks over 1 500 recent transcripts
        // landed right after a line number. A name, not a value; base64
        // punctuation (`/ + =`) keeps the masking, no name carries it.
        if s[whole.end()..].starts_with(": ") && !val.contains(['/', '+', '=']) {
            return whole.as_str().to_string();
        }
        // The value REPEATS its own key: `"token-efficient-tools-2025-02-19":
        // "token-efficient-tools-2025-02-19"`. That shape is a name mapped to
        // itself, and a configuration file of feature flags is nothing but that
        // — five of the twelve entropy hits in the sample were this one file.
        // No credential is the name of the field that holds it.
        let key_tail = before
            .trim_end_matches(['"', '\'', ' ', '\t', ':', '='])
            .trim_end_matches(['"', '\'']);
        if key_tail.ends_with(val) && !val.is_empty() {
            return whole.as_str().to_string();
        }
        // The key NAMES an identifier: `"member_id":"ACoAAB8x…"`,
        // `"profilePathSegment":"ACoAAA…"`, `"conversation_id":"2-YjQ1…"`.
        // An id is what the agent must quote back to act on the record;
        // measured 2026-10-03 over this machine's stash, 935 of 3 163 masks
        // were such fields. Bare connector only, and never a key that also
        // says session/token/secret/key/auth (a session id IS a credential).
        if !pre.chars().next().is_some_and(char::is_alphabetic) && key_names_an_identifier(key_tail)
        {
            return whole.as_str().to_string();
        }
        // The NEXT FLAG is not this key's value: `--annotation=channel=
        // --annotation=lsb-release=Archlinux` (Chrome's crashpad handler in a
        // real `ps auxww`, 2026-10-03) — `channel` is empty and the regex ran
        // on into the following argument. A credential never opens on `--`.
        if val.starts_with("--") {
            return whole.as_str().to_string();
        }
        // A JVM option: `-XX:MaxGCPauseMillis=200`, `-XX:+UseG1GC`. Same
        // treatment as a Cargo directive below — the option NAME is spared,
        // what follows its `=` goes through this pass again.
        if pre == ":" && before.ends_with("-XX") {
            let (name, rest) = val.split_once('=').unwrap_or((val, ""));
            let bare = name.trim_start_matches(['+', '-']);
            if bare.starts_with(|c: char| c.is_ascii_uppercase())
                && bare.chars().all(|c| c.is_ascii_alphanumeric())
            {
                return if val.contains('=') {
                    format!("{pre}{}", reexamined(name, rest))
                } else {
                    whole.as_str().to_string()
                };
            }
        }
        // A Cargo build-script directive: `cargo:rustc-env=BUILD_ID=3f9a…`,
        // `cargo:warning=…`. The `cargo:` colon is a protocol prefix and the
        // directive name is no secret — 22 build outputs delivered
        // `cargo:<redacted>` over the last 1 500 agent transcripts
        // (2026-10-03), the warning text with them. Only the directive name
        // is spared: what follows its `=` goes through this pass again, so an
        // `rustc-env=API_KEY=<secret>` still masks.
        if pre == ":" && before.ends_with("cargo") {
            if let Some((directive, rest)) = val.split_once('=') {
                let known = directive.starts_with("rustc-")
                    || directive.starts_with("rerun-if-")
                    || matches!(directive, "warning" | "error" | "metadata");
                if known {
                    // `warning` / `error` carry a MESSAGE, not a value: it is
                    // left to the passes that read text, as any log line is.
                    if matches!(directive, "warning" | "error") {
                        return whole.as_str().to_string();
                    }
                    return format!("{pre}{}", reexamined(directive, rest));
                }
            }
        }
        // The connector closes a colon NAMESPACE, not an assignment:
        // `urn:li:fsd_profile:ACoAAB…`, `arn:aws:secretsmanager:…:secret:Prod…`.
        // An identifier whose segments are joined by `:` reaches this pass
        // with its last segment as the "value" — measured 2026-10-03 over the
        // last 1 500 agent transcripts on this machine: 144 LinkedIn profile
        // URNs delivered `fsd_profile:<redacted>`, the one handle every
        // following API call needed. A credential is assigned to a key; no
        // key is written `a:b:`. Bare connector only, as above.
        let token_before = before
            .rsplit(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '(' | '[' | '{' | ',' | '=')
            })
            .next()
            .unwrap_or("");
        // Same when the keyword is ITSELF a segment (`…:123456789012:secret:`,
        // where `secret` is the ARN's resource type and the value its name):
        // glued on both sides, no space, it assigns nothing.
        let bare_segment = pre.starts_with(':') && token_before.contains(':');
        let keyword_segment =
            token_before.ends_with(':') && pre.ends_with(':') && !pre.contains(char::is_whitespace);
        if (bare_segment || keyword_segment) && !token_before.contains("//") {
            return whole.as_str().to_string();
        }
        // The KEY says the value is a NAME: `FeatureName : Microsoft-Windows-
        // Subsystem-Linux`, `DisplayName : …`, `ServiceName = …`. PowerShell's
        // list view writes every object this way, and a long hyphenated name
        // clears every threshold — measured 2026-10-02 on a real
        // `Get-WindowsOptionalFeature -Online` (Windows 10): 14 feature names
        // of 150 came back `<redacted>`, the WSL and Hyper-V ones among them,
        // and `MicrosoftWindowsPowerShellV2Root` carries a digit the
        // identifier veto below refuses. No credential is a name; on a bare
        // connector only, like the vetoes above — where a secret keyword
        // introduces the value, the mask stays.
        let key = before
            .trim_end()
            .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .next()
            .unwrap_or("");
        if bare_connector && key.to_ascii_lowercase().ends_with("name") {
            return whole.as_str().to_string();
        }
        // The value is an IDENTIFIER being passed through, not a credential:
        // `response_model=LiteLLM_ManagedVectorStoreListResponse,`,
        // `vector_store: LiteLLM_ManagedVectorStoresTable`. A 32-character
        // CamelCase type name carries three character classes (`_` is the
        // third) and clears the entropy floor, so the pass that exists for
        // formats no rule knows was eating type names. Same predicate as the
        // keyword pass, gated on the BARE connector for the same reason
        // the absolute-path veto above is: where a secret keyword introduces
        // the value, the key says what it is and the mask stays.
        let after = s.get(whole.end()..).unwrap_or("");
        if bare_connector && (value_is_identifier_path(val) || value_is_file_path(val, after)) {
            return whole.as_str().to_string();
        }
        if entropy_is_secret(val, ctx) {
            format!("{pre}<redacted>")
        } else {
            whole.as_str().to_string()
        }
    })
    .into_owned()
}
