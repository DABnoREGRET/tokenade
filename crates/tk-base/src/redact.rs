//! Secret redaction for command lines and log labels.
//!
//! Without this module, calling `tokenade proxy curl -H "Authorization: Bearer
//! SECRET_TOKEN" https://api…` would write `Bearer SECRET_TOKEN` into
//! `.tokenade/gain.jsonl` — a log file a user might later commit, share, or
//! tail in a screen-share. Defence-in-depth: redact known secret patterns
//! at the boundary, before the label ever touches disk.
//!
//! Patterns covered (case-insensitive, value-bearing):
//!   * `Bearer <token>`, `Basic <base64>`
//!   * `Authorization: …` header values
//!   * `api_key=…`, `apikey=…`, `--api-key …`, `-H X-API-Key: …`
//!   * `--password …`, `--token …`, `--secret …`
//!   * `password=…`, `token=…`, `secret=…`, `pwd=…`
//!   * `<SOMETHING>_TOKEN=…`, `<SOMETHING>_KEY=…`, `<SOMETHING>_SECRET=…`
//!   * `Cookie: …` (whole header) and `Set-Cookie: NAME=VALUE; attrs…`
//!     (VALUE only — cookie name + attributes preserved)
//!   * AWS-style `AKIA[0-9A-Z]{16}`, GitHub-style `ghp_*`/`github_pat_*`
//!   * many provider prefixes (Stripe/Slack/Shopify/Google/Notion/…),
//!     AWS secret keys by proximity, DSN/`.npmrc`/`.netrc` auth, K8s Secrets,
//!     GCP service accounts, PuTTY keys, payment-card PANs (Luhn), PII
//!     (email/phone/IBAN/SSN), and a generic high-entropy fallback.
//!     PII / entropy / PAN passes each have an env opt-out
//!     (`TOKENADE_NO_REDACT_{PII,ENTROPY,PAN}`).
//!
//! The redaction REPLACES the value with `<redacted>` rather than dropping
//! the surrounding context — the agent still sees "Authorization: <redacted>"
//! and can reason about the call, just not exfiltrate the credential.

mod cards;
mod context_gate;
mod keyword_assignments;
mod rules;
use cards::*;
use context_gate::*;
use keyword_assignments::*;
use regex::{Captures, Regex};
use std::sync::OnceLock;

// Seven subjects, and two of them contradict each other on purpose: `entropy` judges a
// string WITHOUT context, `structured` judges the PLACE of a value without looking at
// its shape. Keeping them apart is what stops one from inheriting the other's
// criterion.
mod cookies;
mod core;
mod entropy;
mod ledger;
mod pii;
mod structured;
mod values;
pub use cookies::*;
pub use core::*;
use entropy::*;
pub use ledger::*;
use pii::*;
pub use structured::*;
use values::*;

#[cfg(test)]
include!("redact_tests.rs");
