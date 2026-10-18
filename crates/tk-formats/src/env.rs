//! Environment-variable dump compaction (+ secret redaction).
//!
//! `env`, `printenv`, `docker exec … env`, `.env` files: a line per
//! `KEY=VALUE` (or `KEY VALUE` for `printenv` on some shells). The signal is
//! the **keys**; long values like `PATH` (50+ entries), `LS_COLORS`,
//! `NPM_CONFIG_*`, or `GPG_*` are noise the agent rarely reads.
//!
//! Two jobs:
//!
//!   1. **Compact** — long values are truncated to a marker showing
//!      `first 60 chars…[+N chars]` (N = what was cut), the key remains intact.
//!   2. **Redact** — values of keys that look like secrets
//!      (`*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*PASSWD*`, `*API_KEY*`,
//!      `*PRIVATE_KEY*`, `*AUTH*`, `*BEARER*`, `*COOKIE*`, …) get fully
//!      replaced by `<redacted N chars>` so an agent reading the output
//!      never sees the raw secret.
//!
//! Defence-in-depth: even if the agent's output is logged, cached, or sent
//! through another tool, the secret is already gone by the time `auto`
//! decides to print.

use std::fmt::Write;

/// Values longer than this are truncated to a head + length marker.
pub const VALUE_CAP: usize = 60;

/// Case-insensitive substrings that mark a key as secret-bearing.
///
/// Inspired by caveman's `compress.py` denylist (14 extensions + 5
/// token forms) and rtk's vendor-specific patterns. We use substring
/// matching so a new `*_TOKEN` env var is automatically redacted.
const SECRET_MARKERS: &[&str] = &[
    // Generic.
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "PWD", // /etc/passwd is plural, ok — but PWD env var holds the cwd.
    // We refine below: PWD alone is NOT redacted, only PWD-containing keys.
    "API_KEY",
    "APIKEY",
    "PRIVATE_KEY",
    "PRIVATEKEY",
    "AUTH",
    "BEARER",
    "COOKIE", // Cookie/Set-Cookie/HTTP_COOKIE env vars carry session material.
    "ACCESS_KEY",
    "ACCESSKEY",
    "SESSION",
    "SIGNATURE",
    "CREDENTIAL",
    "ENCRYPTION_KEY",
    "OAUTH",
    "WEBHOOK",
    "LICENSE_KEY",
    "SECRET_KEY",
    // Vendor-specific (high-traffic patterns).
    "SLACK_WEBHOOK",
    "STRIPE_KEY",
    "STRIPE_SECRET",
    "AWS_SECRET",
    "AWS_ACCESS_KEY_ID",
    "AZURE_CLIENT_SECRET",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GITLAB_TOKEN",
    "GCP_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "DIGITALOCEAN_TOKEN",
    "LINODE_TOKEN",
    "HEROKU_API_KEY",
    "FLY_API_TOKEN",
    "VERCEL_TOKEN",
    "NETLIFY_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "COHERE_API_KEY",
    "HF_TOKEN",
    "HUGGINGFACE_TOKEN",
    "DATABASE_URL",
    "DATABASE_PASSWORD",
    "DB_URL",
    "DB_PASSWORD",
    "POSTGRES_PASSWORD",
    "POSTGRES_URL",
    "MYSQL_PASSWORD",
    "MYSQL_ROOT_PASSWORD",
    "MONGO_URI",
    "MONGODB_URI",
    "MONGODB_URL",
    "REDIS_URL",
    "REDIS_PASSWORD",
    "RABBITMQ_URL",
    "RABBITMQ_PASSWORD",
    "ELASTICSEARCH_URL",
    "ELASTIC_PASSWORD",
    "DATADOG_API_KEY",
    "DD_API_KEY",
    "DD_APP_KEY",
    "NEW_RELIC_LICENSE_KEY",
    "SENTRY_DSN",
    "SENTRY_AUTH_TOKEN",
    "MAILGUN_API_KEY",
    "SENDGRID_API_KEY",
    "POSTMARK_TOKEN",
    "TWILIO_AUTH_TOKEN",
    "TWILIO_API_KEY",
    "CIRCLECI_TOKEN",
    "BUILDKITE_AGENT_TOKEN",
    "JENKINS_TOKEN",
    "TRAVIS_TOKEN",
    "CODECOV_TOKEN",
    "NPM_TOKEN",
    "PYPI_TOKEN",
    "CARGO_REGISTRY_TOKEN",
    "DOCKER_PASSWORD",
    "DOCKERHUB_TOKEN",
    "SSH_PRIVATE_KEY",
    "GPG_KEY",
    "GPG_PASSPHRASE",
    "JWT_SECRET",
    "JWT_KEY",
    "DSN", // Sentry style
];

/// Keys we explicitly exclude from secret-style redaction even when they
/// match a generic substring (false-positive guard for common env vars).
const NOT_SECRETS: &[&str] = &[
    "PWD",        // current working dir, not a password
    "OLDPWD",     // previous cwd
    "TOKEN_TYPE", // OAuth metadata, not the token itself (the token value is in *_TOKEN)
    "AUTHOR",     // git author
    "AUTHORITY",  // OPA authority etc.
];

/// Keys whose VALUES are typically very long but not secret — we truncate
/// instead of redacting.
const LONG_BUT_PUBLIC: &[&str] = &[
    "PATH",
    "MANPATH",
    "INFOPATH",
    "LD_LIBRARY_PATH",
    "PYTHONPATH",
    "CLASSPATH",
    "LS_COLORS",
    "TERMINFO_DIRS",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    Redact,
    Truncate,
    KeepIfShort,
}

/// Does this environment variable NAME mark a secret-bearing value?
///
/// The single source of truth for the question, for every compactor that
/// renders environment variables. `cmd::linux::env` used to carry its own
/// 16-entry list, which knew `PRIVATE_KEY` and `DATABASE_URL` but not
/// `ANTHROPIC_API_KEY` or `SENTRY_AUTH_TOKEN`, and had no false-positive
/// guard at all — so `PWD` (the cwd) and `AUTHOR` came out redacted there
/// while a `HEROKU_API_KEY` came out in clear. Two lists answering one
/// question drift, and the drift is silent in both directions.
pub fn is_secret_key(key: &str) -> bool {
    let upper = key.to_uppercase();
    if NOT_SECRETS.contains(&upper.as_str()) {
        return false;
    }
    SECRET_MARKERS.iter().any(|m| upper.contains(m))
}

fn classify(key: &str) -> Disposition {
    let upper = key.to_uppercase();
    if is_secret_key(key) {
        return Disposition::Redact;
    }
    if LONG_BUT_PUBLIC.contains(&upper.as_str()) {
        return Disposition::Truncate;
    }
    Disposition::KeepIfShort
}

/// `head…[+N chars]`, N being what was CUT. It used to print the value's total
/// length as `…[+N total]`: read as "N more" it overstated the cut, and `wrap`
/// recognises a cut value only by the `…[+N chars]` every other compactor
/// writes — so a 300-char `PATH`, too small to clear the materiality gate on
/// its own, came back cut with no way to the rest.
fn cut_marker(head: &str, val_len: usize) -> String {
    format!("{head}…[+{} chars]", val_len - head.chars().count())
}

/// Compact an env dump. Detects `KEY=VALUE` (newline-separated) or `KEY VALUE`
/// (single space) format; routes unknown text through unchanged.
pub fn compact(text: &str) -> String {
    if !looks_like_env(text) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() / 2);
    let mut redacted = 0usize;
    let mut truncated = 0usize;
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = split_env_line(raw) else {
            // Pass through lines that don't parse — agent may want to see
            // them (warnings, comments, banners).
            out.push_str(raw);
            out.push('\n');
            continue;
        };
        let disposition = classify(key);
        let val_len = value.chars().count();
        let new_value = match disposition {
            Disposition::Redact => {
                redacted += 1;
                format!("<redacted {val_len} chars>")
            }
            Disposition::Truncate => {
                if val_len > VALUE_CAP {
                    truncated += 1;
                    let head: String = value.chars().take(VALUE_CAP).collect();
                    cut_marker(&head, val_len)
                } else {
                    value.to_string()
                }
            }
            Disposition::KeepIfShort => {
                if val_len > VALUE_CAP * 4 {
                    truncated += 1;
                    let head: String = value.chars().take(VALUE_CAP).collect();
                    cut_marker(&head, val_len)
                } else {
                    value.to_string()
                }
            }
        };
        out.push_str(key);
        out.push('=');
        out.push_str(&new_value);
        out.push('\n');
    }
    if redacted > 0 || truncated > 0 {
        let _ = writeln!(
            out,
            "# [tokenade:env] {redacted} secret(s) redacted, {truncated} long value(s) truncated"
        );
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// Heuristic: ≥3 of the first 10 non-empty lines look like `KEY=value` or
/// `KEY value`, with KEY in shell-variable form (uppercase + underscores +
/// digits).
fn looks_like_env(text: &str) -> bool {
    // A shebang means this is a SCRIPT, and this compactor is a value
    // redactor: it replaces the right-hand side of anything that looks like a
    // secret and truncates anything long. On `env` output that is exactly
    // right; on a shell script it hides code and cuts executable lines in
    // half. `env`, `printenv` and a `.env` file never begin with `#!`.
    //
    // Measured 2026-08-26 over this corpus: 59 files were claimed as an env
    // dump and **55 of them are `.sh` scripts** — four came back with a
    // `<redacted …>` where their source had a command substitution, and one
    // with a line truncated mid-statement. A script's opening reaches the
    // three `KEY=value` lines this function asks for without trying.
    if text.trim_start().starts_with("#!") {
        return false;
    }
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(10)
        .collect();
    if lines.is_empty() {
        return false;
    }
    let envish = lines.iter().filter(|l| split_env_line(l).is_some()).count();
    envish >= lines.len().min(3)
}

fn split_env_line(line: &str) -> Option<(&str, &str)> {
    // `KEY=value` (most common, env and .env).
    if let Some((key, value)) = line.split_once('=') {
        if is_shell_var(key) {
            return Some((key, value));
        }
    }
    // `KEY value` (printenv on some BSD shells).
    if let Some((key, value_raw)) = line.split_once(' ') {
        if is_shell_var(key) {
            return Some((key, value_raw.trim_start()));
        }
    }
    None
}

fn is_shell_var(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().any(|c| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_env_dump() {
        let t = "PATH=/usr/bin:/bin\nHOME=/home/me\nTERM=xterm-256color\n";
        assert!(looks_like_env(t));
        let out = compact(t);
        assert!(out.contains("HOME=/home/me"));
        assert!(out.contains("TERM=xterm-256color"));
    }

    #[test]
    fn redacts_secrets() {
        let t = "PATH=/usr/bin\n\
                 OPENAI_API_KEY=sk-1234567890abcdef1234567890abcdef\n\
                 GITHUB_TOKEN=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
                 AWS_SECRET_ACCESS_KEY=verysecretvaluehere\n\
                 SLACK_WEBHOOK_URL=https://hooks.slack.com/services/T0/B0/abcdef\n\
                 HOME=/home/me\n";
        let out = compact(t);
        assert!(out.contains("HOME=/home/me"));
        // Secret values must NOT appear in the output.
        assert!(!out.contains("sk-1234567890abcdef"));
        assert!(!out.contains("ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(!out.contains("verysecretvaluehere"));
        // But the KEY names are still visible.
        assert!(out.contains("OPENAI_API_KEY="));
        assert!(out.contains("GITHUB_TOKEN="));
        assert!(out.contains("AWS_SECRET_ACCESS_KEY="));
        assert!(out.contains("redacted"));
        // Summary footer present.
        assert!(out.contains("secret(s) redacted"));
    }

    #[test]
    fn truncates_long_public_values() {
        let path = (0..50)
            .map(|i| format!("/very/long/path/dir-{i}"))
            .collect::<Vec<_>>()
            .join(":");
        let t = format!("PATH={path}\nHOME=/me\n");
        let out = compact(&t);
        assert!(out.contains("PATH="));
        // The marker counts what was CUT, in the form `wrap` reads as a cut.
        let cut = path.chars().count() - VALUE_CAP;
        assert!(out.contains(&format!("…[+{cut} chars]")), "{out}");
        assert!(out.contains("long value(s) truncated"));
        assert!(out.contains("HOME=/me"));
    }

    #[test]
    fn pwd_not_redacted() {
        // PWD contains "PWD" substring of "PASSWORD" markers — but PWD itself
        // is the current working directory, not a secret. NOT_SECRETS guard.
        let t = "PWD=/home/me/projects/tokenade\nOLDPWD=/home/me\nHOME=/home/me\n";
        let out = compact(t);
        assert!(out.contains("PWD=/home/me/projects/tokenade"));
        assert!(out.contains("OLDPWD=/home/me"));
        assert!(!out.contains("redacted"));
    }

    #[test]
    fn unknown_input_passes_through() {
        let t = "this is not an env dump at all\nit's prose\n";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_real_shape() {
        // 60-entry PATH + GPG_KEY + a couple normal vars.
        let mut t = String::new();
        let path = (0..60)
            .map(|i| format!("/usr/local/path-{i}/bin"))
            .collect::<Vec<_>>()
            .join(":");
        let _ = writeln!(t, "PATH={path}");
        let _ = writeln!(t, "GPG_PRIVATE_KEY={}", "abcdef0123456789".repeat(40));
        t.push_str("HOME=/me\nUSER=me\nTERM=xterm\n");
        let (out, sav) = compact_with_savings(&t);
        assert!(out.contains("HOME=/me"));
        assert!(!out.contains("abcdef0123456789abcdef0123456789")); // redacted
        assert!(sav.percent() > 75.0, "got {:?}", sav);
    }
}

#[cfg(test)]
mod a_shell_script_is_not_an_env_dump {
    //! This compactor REDACTS values and TRUNCATES long ones. On `env` output
    //! that is the whole point; on a shell script it hides code and cuts
    //! statements in half. Measured over this corpus: 59 files claimed,
    //! **55 of them `.sh`** — four came back with a `<redacted …>` in place of
    //! a command substitution, one with a truncated line.

    const SCRIPT: &str = "#!/usr/bin/env bash\nset -euo pipefail\nROOT_DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nLOG_FILE=/tmp/run.log\nMODE=fast\nAPI_SECRET=\"$(cat /run/secrets/api)\"\necho \"$ROOT_DIR\"\n";

    #[test]
    fn a_script_is_left_alone() {
        assert_eq!(super::compact(SCRIPT), SCRIPT, "a script is not a dump");
    }

    // The two tests that queried the ROUTER (`auto::detect` on a
    // script and on a `.env`) are in
    // `tests/misc/a_format_is_reached_by_its_router.rs`.

    #[test]
    fn a_real_env_dump_is_still_redacted() {
        let dump = "PATH=/usr/bin:/bin\nHOME=/home/me\nAPI_SECRET=abcdefghijklmnopqrstuvwxyz\nLANG=en_US.UTF-8\n";
        let out = super::compact(dump);
        assert!(out.contains("<redacted "), "secrets must still go: {out}");
        assert!(out.contains("PATH="), "ordinary values stay");
    }
}
