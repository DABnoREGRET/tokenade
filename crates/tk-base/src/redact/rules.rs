//! The redaction rule TABLE: `(regex, replacement)` rows, ordered specific →
//! general so a broad rule never swallows a narrow one.
//!
//! Data, not logic. `redact::patterns()` compiles each row lazily behind its
//! own `OnceLock`, and every rule's rationale travels with the row it
//! justifies. Split out for the same reason as `index/lang_specs.rs`: a
//! 400-line literal is unreadable inside the module that consumes it, and the
//! two halves never change together.

/// Ordered `(pattern, replacement)` rows. `${1}` back-references keep the
/// non-secret prefix (scheme, key name) so the reader still sees WHAT was
/// redacted.
pub(super) static RULES: &[(&str, &str)] = &[
    // HTTP auth schemes (value after the scheme word, up to whitespace/quote).
    // Ordered specific → general so Bearer/Basic don't get swallowed
    // by a broader Authorization match.
    // NOTE: bare `Bearer <x>` / `Basic <x>` are handled by
    // `redact_bare_auth_schemes`, which gates on the SHAPE of the
    // value. As plain regex rules they ate documentation prose — see
    // that function.
    // B — connection-string URIs. Unlike the generic userinfo rule
    // below (which keeps user + host and masks only the password), a
    // database / broker DSN leaks infra that is itself sensitive: the
    // whole AUTHORITY (`user:pwd@host:port`) is redacted for these
    // schemes, keeping only the scheme + path so the URI stays
    // grep-able. Placed BEFORE the generic userinfo rule so it wins.
    // Requires `user:pwd@host` (a `:` then `@` before any `/`), so a
    // credential-less `redis://host` is untouched. Idempotent
    // (`<redacted>` has no `:`…`@`).
    (
        // The user half is `*`, not `+`: Redis has no username in the
        // classic form, so its credentialed URL is
        // `redis://:password@host` — the shape this rule was written
        // for, and the one it missed. MEASURED 2026-08-04: postgres://
        // and mongodb+srv:// were redacted, redis:// went through with
        // its password in clear. Requiring a `:` before the `@` still
        // keeps a credential-less `redis://host` untouched.
        r#"(?i)\b((?:mongodb(?:\+srv)?|mysql|postgres(?:ql)?|redis|rediss|amqp|amqps|ftp)://)[^\s/@]*:[^\s/@]+@[^\s/?#"'`]+"#,
        "${1}<redacted>",
    ),
    // URL userinfo `scheme://user:password@host` (#476). RFC 3986
    // allows credentials inline; common in legacy CI scripts +
    // db/redis/mongo URIs. Capture the scheme + user so the
    // structure stays grep-able; redact only the password half.
    // Userinfo chars per RFC: unreserved + pct-encoded + sub-delims
    // + `:` (only one `:` separates user / pwd). We're conservative
    // and accept printable non-`/`, non-`@`, non-`:` for both.
    (
        // `*` on the user half for the same reason as the DSN rule above:
        // `scheme://:password@host` is a credential with no username.
        r#"([A-Za-z][A-Za-z0-9+.\-]*://[^:@/\s"'`]*):([^@/\s"'`]+)@"#,
        "${1}:<redacted>@",
    ),
    // `X-Api-Key: ...`, `X-Auth-Token: ...`
    (
        r#"(?i)(X-[A-Za-z\-]*(?:API[_\-]?KEY|AUTH[_\-]?TOKEN|TOKEN|SECRET)\s*:\s*)([^\s"'`]+)"#,
        "${1}<redacted>",
    ),
    // `key=value` style env / query params with secret-looking keys.
    // The value alternation accepts a single-quoted string, a
    // double-quoted string, OR a bare token. Production leak: a quoted secret value escaped the
    // original bare-only matcher
    // `[^\s"'`&]+` stops at the opening quote of `KEY='secret'`,
    // so quoted assignments escaped redaction entirely. Matching
    // the quoted forms first plugs that hole for any secret value
    // (the quotes are dropped together with the value).
    // C7 audit — YAML/JSON colon forms (config dumps cross the stash
    // threshold constantly; = -only rules missed them). Ordered after
    // URL userinfo so scheme://u:p@ is consumed first.
    // C7 — generic Authorization schemes beyond Bearer/Basic
    // (Token/Digest/SSWS/…): header value fully redacted.
    //
    // The OPENING QUOTE is part of the prefix, not of the value. Without it
    // the scheme alternative could not match a header written as a string —
    // `Authorization: "Bearer special-token"` — so the SECOND alternative took
    // it, `\S+` ate `"Bearer` and the replacement produced
    // `Authorization: <redacted> special-token"`. The scheme word was masked
    // and the TOKEN was left in clear: the rule fired inverted on the
    // majority form. Measured 2026-08-28 over 2 000 third-party source files:
    // 49 of the 168 provider masks are this shape.
    //
    // The value class stops at a quote for the same reason: the closing quote
    // belongs to the source, not to the secret, so the line stays valid
    // (`Authorization: "Bearer <redacted>"`). And an interpolation
    // (`\`Bearer ${apiKey}\``) now reaches the placeholder veto in
    // `apply_provider_patterns` with the VALUE in hand, which spares it.
    (
        r#"(?i)((?:Proxy-)?Authorization\s*:\s*['"`]?(?:Bearer|Basic|Token|Digest|SSWS|GenieKey)\s+|(?:Proxy-)?Authorization\s*:\s*['"`]?)(?:<redacted>|[^\s'"`]+)"#,
        "${1}<redacted>",
    ),
    // Cookie headers. The whole `Cookie:` request header is a bag of
    // session material — redact everything after the colon. Ordered
    // and anchored so it does NOT swallow the `Cookie:` substring
    // inside `Set-Cookie:` (regex crate has no lookbehind, so we
    // capture the preceding non-word/non-dash char — or line start —
    // and put it back). Idempotent: `Cookie: <redacted>` re-redacts
    // to itself.
    // The preceding-character class also excludes `:`, because `::` is a PATH
    // separator and this rule eats everything to end of line. `reqwest::cookie::
    // CookieStore` in a Rust symbol list took the whole line with it, and so does
    // any `cookie` module in C++ or Rust. Measured 2026-09-02 over one machine's
    // real tool output: 13 of the 38 places this rule can fire are that shape —
    // a third of its hits, each one destroying a line it has no business in.
    // A genuine header never has a colon immediately before its name.
    (
        r"(?i)(^|[^-\w:])(Cookie\s*:\s*)[^\r\n]*",
        "${1}${2}<redacted>",
    ),
    // A JSON body carrying the header as a FIELD: `"cookie": "session=…"`.
    // The rule above cannot cover it — it eats to end of line, which on a
    // one-line JSON object would take every field after the cookie with it —
    // so this one is bounded by the closing quote and leaves the document
    // parseable. Found 2026-09-02 while checking what the header rule covers:
    // `{"headers":{"cookie":"session=abc"}}` came back with the value intact,
    // and a request body is exactly where a captured cookie lives.
    (r#"(?i)("cookie"\s*:\s*")[^"\r\n]*""#, "${1}<redacted>\""),
    // `Set-Cookie: NAME=VALUE; attrs…` — redact ONLY the value up to
    // the first `;`, keeping the cookie name and every attribute
    // (Path/Domain/HttpOnly/…) visible so the agent can still reason
    // about scope/flags. Idempotent (VALUE `<redacted>` → `<redacted>`).
    (
        r"(?i)(Set-Cookie\s*:\s*[^=;\r\n]+=)([^;\r\n]*)",
        "${1}<redacted>",
    ),
    // C7 — sshpass/-p on the STRING path (redact_argv catches argv,
    // but transcript lines uploaded via report flow through redact()).
    (r"(?i)\bsshpass\s+-p\s*\S+", "sshpass -p <redacted>"),
    // B — PGP private key block. The PEM rule below requires
    // `PRIVATE KEY-----` immediately, but the PGP header is
    // `PRIVATE KEY BLOCK-----` (extra word), so PGP escaped it.
    // Anchored on the exact armor lines; runs first.
    (
        r"(?is)-----BEGIN PGP PRIVATE KEY BLOCK-----.*?-----END PGP PRIVATE KEY BLOCK-----",
        "<redacted-private-key>",
    ),
    // C7 — PEM private keys: whole blob when the text is one string,
    // header alone on line-by-line paths.
    (
        r"(?is)-----BEGIN[A-Z0-9 ]*PRIVATE KEY-----.*?-----END[A-Z0-9 ]*PRIVATE KEY-----",
        "<redacted-private-key>",
    ),
    // …and when the END marker is NOT there. It often is not: every
    // compactor clips long values, and clipping runs BEFORE redaction,
    // so a key embedded in a JSON string arrived as
    // `-----BEGIN…-----\nMIIEow…\n-----END RSA PRIVATE KEY-…[+11 chars]`.
    // The old rule replaced the HEADER and left the key material in
    // clear — worse than doing nothing, because the output then carries
    // `<redacted-private-key>` and reads as handled.
    //
    // So consume the base64 body too, bounded to runs of 20+ base64
    // characters: ordinary prose stops the match at its first short
    // word, and a truncated END marker is swallowed by the tail.
    (
        r"(?i)-----BEGIN[A-Z0-9 ]*PRIVATE KEY-----(?:(?:\s|\\r|\\n)*[A-Za-z0-9+/=]{20,})*(?:(?:\s|\\r|\\n)*-----END[A-Z0-9 ]*PRIVATE\s*K?E?Y?-*)?",
        "<redacted-private-key>",
    ),
    // E — PuTTY private key (.ppk). The OpenSSH new-format
    // `-----BEGIN OPENSSH PRIVATE KEY-----` is already caught by the
    // PEM rule above (OPENSSH ∈ [A-Z0-9 ]*). PuTTY keys are NOT PEM:
    // redact the `Private-Lines:` body up to the `Private-MAC:` line.
    (
        r"(?is)(Private-Lines:\s*\d+\s*[\r\n]+).*?([\r\n]+\s*Private-MAC\s*:)",
        "${1}<redacted-private-key>${2}",
    ),
    // G — GCP service-account JSON. `private_key` is PEM (above);
    // `private_key_id` and `client_secret` are keyed values.
    (
        r#"(?i)("(?:private_key_id|client_secret)"\s*:\s*")[^"]*"#,
        "${1}<redacted>",
    ),
    // C — ADO.NET / ODBC / JDBC connection-string DSN. `;Password=x;`
    // / `;Pwd=x;` — redact the value up to the next `;` (or EOL),
    // keeping the surrounding `Server=…;Database=…` keys visible.
    // Placed BEFORE the generic `password=` rule (whose bare matcher
    // would greedily eat the rest of the DSN) so `;`-termination wins.
    (
        // No space before `=`, for the same reason as the keyword rules
        // above: a DSN writes `Password=x;Server=y`, code writes
        // `let password = expr;` — and this rule's value class runs to
        // the next `;`, so on code it swallowed a whole statement.
        r#"(?i)((?:^|[;\s])\s*(?:password|pwd)=)[^;\r\n]*"#,
        "${1}<redacted>",
    ),
    // D — registry auth files. `.npmrc` `_authToken=…` / `_auth=…` /
    // `:_password=…` are lowercase; the uppercase `_TOKEN=` rule below
    // misses them. Idempotent (value → `<redacted>`).
    // D — `.netrc`: `machine H login U password P`. Space-separated
    // (no `=`). Strictly anchored on `machine … login … password …`
    // so English prose ("the password is …") never fires.
    (
        r#"(?i)(\bmachine\s+\S+\s+login\s+)\S+(\s+password\s+)\S+"#,
        "${1}<redacted>${2}<redacted>",
    ),
    // Bare value class also excludes `;` so a DSN
    // `Password=x;Server=y` stops at the separator (keeps `Server=y`
    // visible) instead of swallowing the rest of the connection string.
    // Env-var-looking secrets: `FOO_TOKEN=...`, `FOO_KEY=...`, `FOO_SECRET=...`.
    (
        // Case-SENSITIVE, unlike its neighbours, and that is the whole point.
        // Under `(?i)` this matched ANY identifier ending in `_token` / `_key`
        // / `_secret`, so reading ordinary source code destroyed the right-hand
        // side of every such assignment: `let kind_token = value_kind_token(o)`
        // came back masked, and so did `let api_key = build_key(cfg)`. Agents
        // read code all day and then EDIT against what they read, so a silently
        // corrupted line is worse than a verbose one.
        //
        // What this gives up: a SHORT (<32 char) secret behind a lowercase name.
        // Real credentials are long, and those stay covered by the generic
        // keyword-then-32-credential-chars rule below - which cannot fire on an
        // expression, since a call contains `(` and that is not in its value
        // class. SCREAMING_SNAKE is what an env var looks like everywhere, and
        // it is what this rule was written for.
        r#"(\b[A-Z][A-Z0-9_]*(?:_TOKEN|_KEY|_SECRET|_API_KEY|_PASSWORD|_PASS|_PASSWD|_PWD)\s*=\s*)(?:'[^']*'|"[^"]*"|[^\s"'`&]+)"#,
        "${1}<redacted>",
    ),
    // B — AWS Secret Access Key: 40 chars of base64 WITHOUT a
    // distinctive prefix. Redacted ONLY by proximity to a
    // `aws_secret_access_key` / `aws_secret` key name or a
    // `--secret-access-key` flag on the same line/object — never a
    // bare 40-char blob (which would false-positive on every base64
    // chunk / hash). The `AWS_SECRET_ACCESS_KEY=` env form is also
    // caught by the generic `_KEY=` rule; this adds the lowercase
    // JSON/YAML colon form and the CLI flag.
    (
        r#"(?i)(aws[_\-]?secret(?:[_\-]?access[_\-]?key)?\s*[:=]\s*["']?)[A-Za-z0-9/+]{40}"#,
        "${1}<redacted>",
    ),
    (
        r#"(?i)(--secret-access-key[=\s]+["']?)[A-Za-z0-9/+]{40}"#,
        "${1}<redacted>",
    ),
    // Cloudflare legacy Global API Key — 37-hex, keyed by proximity
    // (a bare 37-40 hex run is indistinguishable from a hash).
    (
        r#"(?i)((?:cf[_\-]?api[_\-]?key|cloudflare[_\-]?api[_\-]?key|x-auth-key)\s*[:=]\s*["']?)[a-f0-9]{37,40}"#,
        "${1}<redacted>",
    ),
    // Known secret-token formats with very high specificity.
    // AWS access keys — full documented prefix set (access/temp/user/
    // role/instance-profile/…), 16 base32 chars after the 4-char tag.
    // Formats from CodeGate signatures.yaml (Apache) / AWS docs.
    //
    // Bounded on both sides by anything that is NOT an uppercase letter or a
    // digit, because the key's own alphabet is exactly that: a 26-character
    // Docker overlay layer id (`/var/lib/docker/overlay2/l/MKANVAUC73HU7…`) is
    // the same base32 run, and `ANVA` + 16 inside it was redacted — `mount`
    // came back with `MK<redacted>ROL4`, a path that no longer exists. A real
    // key stands alone (`=AKIA…`, `"AKIA…"`, `AKIA…\n`). Lowercase and `_` are
    // deliberately NOT boundaries-required: `key_AKIA…` still redacts.
    (
        r"(^|[^A-Z0-9])((?:A3T[A-Z0-9]|AKIA|AGPA|AIDA|AROA|AIPA|ANPA|ANVA|ASIA|ABIA|ACCA)[A-Z0-9]{16})([^A-Z0-9]|$)",
        "${1}<redacted>${3}",
    ),
    // AWS Marketplace (MWS) auth token — `amzn.mws.<uuid>`.
    (r"(?i)amzn\.mws\.[0-9a-f-]{36}", "<redacted>"),
    // AWS AppSync GraphQL API key — `da2-<26 base36>`.
    (r"\bda2-[a-z0-9]{26}\b", "<redacted>"),
    // GitHub PATs / fine-grained / OAuth / refresh / user-to-server.
    (r"ghp_[A-Za-z0-9]{30,}", "<redacted>"),
    (r"github_pat_[A-Za-z0-9_]{50,}", "<redacted>"),
    (r"gho_[A-Za-z0-9]{30,}", "<redacted>"),
    (r"ghs_[A-Za-z0-9]{30,}", "<redacted>"),
    (r"ghr_[A-Za-z0-9]{30,}", "<redacted>"),
    (r"ghu_[A-Za-z0-9]{30,}", "<redacted>"),
    // Slack tokens.
    // Slack tokens open on a DIGIT after the prefix (`xoxb-<team id>-…`,
    // `xapp-1-…`, `xoxe-1-…`). The bare prefix matched package names:
    // `xapp-symbolic-icons` (Arch/Mint) came back `<redacted>` in a real
    // `pacman -Q` (2026-10-03).
    (r"xox[abprs]-\d[A-Za-z0-9\-]{9,}", "<redacted>"),
    // Stripe keys (#474). live/test/restricted all leak the
    // same way — test mode still touches real customer data
    // in many setups, and restricted keys (rk_*) can be more
    // dangerous because they're often given broader perms by
    // teams that didn't realise.
    (r"sk_live_[A-Za-z0-9]{20,}", "<redacted>"),
    (r"sk_test_[A-Za-z0-9]{20,}", "<redacted>"),
    (r"rk_live_[A-Za-z0-9]{20,}", "<redacted>"),
    (r"rk_test_[A-Za-z0-9]{20,}", "<redacted>"),
    // GitLab personal access / service account / runner tokens (#474).
    (r"glpat-[A-Za-z0-9_\-]{20,}", "<redacted>"),
    (r"glsoat-[A-Za-z0-9_\-]{20,}", "<redacted>"),
    (r"glrt-[A-Za-z0-9_\-]{20,}", "<redacted>"),
    // Twilio account SID + auth-token SID (#474). Exactly 32
    // lowercase hex chars after the 2-letter prefix. Anchored
    // with word boundaries so they don't false-positive on the
    // common substrings "ACK" / "ACK_" / "SKIP" etc.
    (r"\bAC[a-f0-9]{32}\b", "<redacted>"),
    (r"\bSK[a-f0-9]{32}\b", "<redacted>"),
    // SendGrid (#474) — strict `SG.<22>.<43>` format.
    (r"SG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}", "<redacted>"),
    // Mailgun (#474) — `key-<32 hex>`.
    (r"\bkey-[a-f0-9]{32}\b", "<redacted>"),
    // Postman API keys / access tokens (#474).
    (r"PMAK-[a-f0-9]{24}-[a-f0-9]{34}", "<redacted>"),
    (r"PMAT-[a-f0-9]{40}", "<redacted>"),
    // Generic high-entropy JWT-shaped.
    (
        r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b",
        "<redacted>",
    ),
    // LLM provider keys (coding agents see these constantly:
    // env dumps, config files, CI logs, MCP-server bootstrap).
    // Anthropic — `sk-ant-…` with 90+ chars of body.
    (r"sk-ant-[A-Za-z0-9_\-]{40,}", "<redacted>"),
    // OpenAI — high-confidence rule anchored on the fixed `T3BlbkFJ`
    // infix that every OpenAI key (`sk-…`, `sk-proj-…`, service /
    // session keys) carries mid-body. This catches keys the length
    // rule misses (project keys contain `-`/`_`) and, being anchored
    // on a distinctive literal, has ~0 FP regardless of length.
    (r"sk-[A-Za-z0-9_\-]*T3BlbkFJ[A-Za-z0-9_\-]*", "<redacted>"),
    // OpenAI — `sk-` followed by 40+ chars (project keys are
    // longer). Anthropic match first (prefix is sk-ant-) so
    // we won't double-replace.
    (r"sk-[A-Za-z0-9]{40,}", "<redacted>"),
    // Google API keys — fixed 35 chars after `AIza`.
    (r"AIza[A-Za-z0-9_\-]{35}", "<redacted>"),
    // HuggingFace user tokens — fixed 34 chars after `hf_`.
    (r"hf_[A-Za-z0-9]{34}", "<redacted>"),
    // npm publish tokens — fixed 36 chars after `npm_`.
    (r"npm_[A-Za-z0-9]{36}", "<redacted>"),
    // DigitalOcean personal access tokens — 64 hex after dop_v1_.
    (r"dop_v1_[a-f0-9]{64}", "<redacted>"),
    // Sentry CLI tokens (sntrys_ prefix, base64-ish body).
    (r"sntrys_[A-Za-z0-9+/=]{40,}", "<redacted>"),
    // Azure Storage SAS signature. Anchored on a QUERY parameter and a
    // long value: `sig` alone is far too common a word to redact on
    // (it is a variable name in half the crypto code in existence), but
    // `?sig=` / `&sig=` followed by 40+ base64/pct-encoded characters is
    // the shared-access signature and nothing else. Grants whatever the
    // rest of the URL describes, so it is a credential in full.
    (r"([?&]sig=)[A-Za-z0-9%+/=_\-]{40,}", "${1}<redacted>"),
    // SendGrid API keys — `SG.<22>.<43>`, both segments fixed width.
    // MEASURED 2026-08-04: the one vendor shape in a 23-secret probe
    // that went through untouched while every neighbour was masked.
    // Anchored on the literal `SG.` plus BOTH exact lengths, so it
    // cannot fire on prose or on a Rust path like `SG.foo`.
    (
        r"\bSG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}\b",
        "<redacted>",
    ),
    // === A — additional provider prefixes (distinctive, ~0 FP) ===
    // OpenAI project-scoped keys (contain `-`/`_`, so the bare `sk-`
    // 40-alnum rule cannot match them).
    (r"sk-proj-[A-Za-z0-9_\-]{20,}", "<redacted>"),
    // Stripe webhook signing secret.
    (r"whsec_[A-Za-z0-9]{20,}", "<redacted>"),
    // Shopify access / shared-secret / custom-app / private-app.
    (r"shp(?:at|ss|ca|pa)_[A-Za-z0-9]{20,}", "<redacted>"),
    // Slack app-level + legacy exchange tokens + incoming-webhook URL.
    (r"xapp-\d-[A-Za-z0-9\-]{10,}", "<redacted>"),
    (r"xoxe-\d-[A-Za-z0-9\-]{10,}", "<redacted>"),
    (
        r"(https://hooks\.slack\.com/services/)\S+",
        "${1}<redacted>",
    ),
    // DigitalOcean OAuth / refresh tokens (dop_v1_ already above).
    (r"doo_v1_[a-f0-9]{64}", "<redacted>"),
    (r"dor_v1_[a-f0-9]{64}", "<redacted>"),
    // Google OAuth2 access (ya29.) + refresh (1//) tokens.
    (r"ya29\.[A-Za-z0-9_\-]{20,}", "<redacted>"),
    (r"\b1//[A-Za-z0-9_\-]{20,}", "<redacted>"),
    // Telegram bot token `<id>:AA<35>`.
    (r"\b\d{8,10}:AA[A-Za-z0-9_\-]{33}\b", "<redacted>"),
    // Discord bot token `<24>.<6>.<27>`.
    (
        r"\b[A-Za-z0-9_\-]{24}\.[A-Za-z0-9_\-]{6}\.[A-Za-z0-9_\-]{27}\b",
        "<redacted>",
    ),
    // New Relic user API key.
    (r"NRAK-[A-Z0-9]{27}", "<redacted>"),
    // Notion integration tokens (legacy `secret_<43>` + new `ntn_`).
    (r"secret_[A-Za-z0-9]{43}", "<redacted>"),
    (r"ntn_[A-Za-z0-9]{40,}", "<redacted>"),
    // Linear API key.
    (r"lin_api_[A-Za-z0-9]{20,}", "<redacted>"),
    // Doppler personal token.
    (r"dp\.pt\.[A-Za-z0-9]{20,}", "<redacted>"),
    // PyPI upload token (fixed `pypi-AgEIcHlwaS` preamble).
    (r"pypi-AgEIcHlwaS[A-Za-z0-9_\-]{10,}", "<redacted>"),
    // age identity (secret) key.
    (r"AGE-SECRET-KEY-1[0-9A-Z]{20,}", "<redacted>"),
    // GitLab deploy / trigger tokens (glpat-/glsoat-/glrt- above).
    (r"gldt-[A-Za-z0-9_\-]{20,}", "<redacted>"),
    (r"glptt-[A-Za-z0-9_\-]{20,}", "<redacted>"),
    // === A/Tier-2 — extended provider prefixes (default ON). ===
    // Token FORMATS re-implemented from CodeGate signatures.yaml
    // (Apache) + proxy-ai scanner (Apache); ~0 FP — each anchors on a
    // distinctive literal prefix/infix. NO code copied, only formats.
    (r"\bgsk_[A-Za-z0-9]{20,}", "<redacted>"), // Groq
    (r"(?i)\bglsa_[A-Za-z0-9]{32}_[a-f0-9]{8}\b", "<redacted>"), // Grafana SA token
    (r"\bdckr_pat_[A-Za-z0-9_=\-]{20,}", "<redacted>"), // Docker Hub PAT
    (
        r"\b[A-Za-z0-9]{14}\.atlasv1\.[A-Za-z0-9_\-]{60,}",
        "<redacted>",
    ), // Terraform Cloud
    (r"(?i)\bsk\.eyj1ijoi[a-z0-9\-_\.]{40,}", "<redacted>"), // Mapbox secret token
    (r"\bfigd_[A-Za-z0-9_\-]{14,}", "<redacted>"), // Figma PAT
    (r"\bvercel_blob_rw_[A-Za-z0-9]{40,}", "<redacted>"), // Vercel Blob RW token
    (r"\bre_[0-9A-Za-z]{24,}\b", "<redacted>"), // Resend
    (
        r"(?i)\bdt0[a-z]\d{2}\.[A-Z0-9]{24}\.[A-Z0-9]{64}\b",
        "<redacted>",
    ), // Dynatrace
    (r"(?i)\boy2[a-z0-9]{43}\b", "<redacted>"), // NuGet
    (r"\bAKCp[0-9][A-Za-z0-9]{50,}", "<redacted>"), // JFrog Artifactory
    (r"(?i)\brubygems_[a-f0-9]{48}\b", "<redacted>"), // RubyGems
    (
        r"\bocid1\.(?:tenancy|user)\.oc1\.[A-Za-z0-9\-_\.]{40,}",
        "<redacted>",
    ), // Oracle OCI
    (
        r"\b(?:pub|sub)-c-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
        "<redacted>",
    ), // PubNub pub/sub keys
    (r"\bAKID[0-9A-Za-z]{16,40}\b", "<redacted>"), // Tencent Cloud
    (r"\baio_[A-Za-z0-9]{28}\b", "<redacted>"), // Adafruit IO
    (
        r"(?i)\brgapi-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
        "<redacted>",
    ), // Riot Games
    (r"\bxkeysib-[a-f0-9]{64}-[A-Za-z0-9]{16}\b", "<redacted>"), // Brevo (Sendinblue)
    (r"(?i)\bwx[a-f0-9]{16}\b", "<redacted>"), // WeChat app secret
    (
        r"access_token\$production\$[0-9a-zA-Z]{28,42}",
        "<redacted>",
    ), // PayPal Braintree
    (r"\bsq0[a-z]{3}-[0-9A-Za-z\-_]{22,43}\b", "<redacted>"), // Square access/app secret
    (r"\bsqOatp-[0-9A-Za-z\-_]{22}\b", "<redacted>"), // Square OAuth
    (
        r"(?i)\brzp_(?:test|live)_[0-9A-Za-z]{10,20}\b",
        "<redacted>",
    ), // Razorpay
    (r"\bFLWSECK-[0-9a-f]{32}-X\b", "<redacted>"), // Flutterwave
    (r"(?i)\bEAA[GC][0-9A-Za-z]{20,}", "<redacted>"), // Facebook access token
    (r"(?i)\bIGQV[0-9A-Za-z\-_]{20,}", "<redacted>"), // Instagram access token
    // Tier-2 breadth — published token formats (detect-secrets upstream,
    // Apache). Only distinctive-prefix providers included; hex/UUID-only
    // providers (Algolia, Heroku, Netlify, Twitch, Zendesk, Okta, …) are
    // deliberately omitted — indistinguishable from a hash → high FP.
    (r"\bATATT3[A-Za-z0-9_\-=]{20,}", "<redacted>"), // Atlassian API token
    (r"\bdapi[a-f0-9]{32}(?:-\d)?\b", "<redacted>"), // Databricks PAT
    (r"\bsl\.[A-Za-z0-9_\-]{130,}", "<redacted>"),   // Dropbox short-lived token
    (
        r"(?i)\bpat-(?:na|eu|ap)1-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
        "<redacted>",
    ), // HubSpot PAT
    (
        r"\bpscale_(?:tkn|pw|oauth)_[A-Za-z0-9_\-\.=]{32,}",
        "<redacted>",
    ), // PlanetScale
    (r"\bCFPAT-[A-Za-z0-9_\-]{40,}", "<redacted>"),  // Contentful CMA token
    (r"\btfp_[A-Za-z0-9_\-\.]{40,}", "<redacted>"),  // Typeform PAT
    (r"(?i)\bp8e-[a-z0-9]{32}\b", "<redacted>"),     // Adobe integration
    (
        r"\b(?:api|sdk|mob)-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
        "<redacted>",
    ), // LaunchDarkly
    (
        r"(?i)(https://[a-z0-9]+\.webhook\.office\.com/webhookb2/)\S+",
        "${1}<redacted>",
    ), // Microsoft Teams incoming webhook
    // GCP — Firebase Cloud Messaging device/legacy key (APA91b infix).
    (
        r"[0-9A-Za-z_\-]{11}:APA91b[0-9A-Za-z_\-]{134}",
        "<redacted>",
    ),
    // GCP — OAuth2 client id (redact the numeric+random id, keep the
    // grep-able suffix so the URL/host stays legible).
    (
        r"[0-9]+-[0-9A-Za-z_]{32}\.apps\.googleusercontent\.com",
        "<redacted>.apps.googleusercontent.com",
    ),
    // GCP — Cloud Storage V2 signed URL: redact the `Signature=` value
    // (gated on the `storage.googleapis.com`+`GoogleAccessId=` shape so
    // it never fires on an unrelated `Signature=` param).
    (
        r#"(?i)(storage\.googleapis\.com/[^\s"'`]*[?&]GoogleAccessId=[^\s"'`]*[?&]Signature=)[^\s&"'`]+"#,
        "${1}<redacted>",
    ),
    // Azure — Storage / Service Bus shared key `AccountKey=<88 base64>`.
    (
        r"(?i)(AccountKey=)[A-Za-z0-9+/]{86,}={0,2}",
        "${1}<redacted>",
    ),
    // B — Bitcoin WIF private key (`5`/`K`/`L` + 50–51 base58). The
    // leading char + fixed length + base58 alphabet make it distinctive
    // enough to keep ON by default. (ETH/XRP private keys are NOT here —
    // they're bare hex/base58 that collide with hashes, so they live in
    // the proximity-gated pass below.)
    (r"\b[5KL][1-9A-HJ-NP-Za-km-z]{50,51}\b", "<redacted>"),
];
