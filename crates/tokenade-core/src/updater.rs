//! Tokenade auto-updater — fetches signed manifest, verifies, swaps binary.
//!
//! Design constraints, in order of importance:
//!
//! 1. **NEVER block the user.** The user invoked `tokenade auto file.json`
//!    to compact a file; they did not invoke an update. The update check
//!    runs in a detached thread; the main thread returns immediately.
//!
//! 2. **NEVER log to stdout/stderr.** The MCP server speaks JSON-RPC on
//!    stdout — a stray println from this module would corrupt the wire
//!    protocol. All status goes through `debug_log::log` (file-only) and
//!    the cache file. Failures are silently swallowed; the next 24h
//!    check will retry.
//!
//! 3. **Atomic swap.** Use a tmp file in the same directory as the live
//!    binary, then `rename(2)`. The CLI may already be running when the
//!    swap happens — that's fine on Unix (rename onto an open file works
//!    because Linux/macOS file handles point at the inode, not the path),
//!    fine on Windows IF the binary isn't currently `tokenade.exe` running
//!    in the foreground (we re-rename the old binary to `.old` first as a
//!    safety net, then rename the new one in).
//!
//! 4. **Signature is the source of truth, NOT TLS.** The manifest is
//!    signed with the same Ed25519 key as the licence blob (per user
//!    decision 2026-06-02 — single keypair for simplicity). Even an
//!    attacker who terminates TLS at the CDN edge can't swap the binary
//!    because the signature won't verify.
//!
//! Cadence:
//!   - First invocation per UTC day checks for updates.
//!   - 24h TTL stored in `~/.tokenade/update-check.json`.
//!   - Disable: `TOKENADE_NO_AUTO_UPDATE=1` (env var, one shot), or
//!     persistently with `tokenade upgrade --off`, which writes the marker
//!     file `~/.tokenade/auto-update-off` (`--on` removes it).
//!
//! Manifest schema (`v=1`):
//! ```json
//! {
//!   "v": 1,
//!   "version": "0.3.0",
//!   "releasedAt": 1780362164,
//!   "minSupportedVersion": "0.2.0",
//!   "targets": {
//!     "x86_64-unknown-linux-musl": {
//!       "url": "https://github.com/pi-infected/tokenade/releases/download/v2.0.0/tokenade-2.0.0-x86_64-unknown-linux-musl.tar.gz",
//!       "sha256": "abcd1234…",
//!       "size": 12345678
//!     },
//!     "aarch64-unknown-linux-musl": { …same shape… },
//!     "x86_64-apple-darwin": { … },
//!     "aarch64-apple-darwin": { … },
//!     "x86_64-pc-windows-msvc": { … }
//!   }
//! }
//! ```
//! Signed by the release pipeline (canonical `JSON.stringify`, fixed key order).
//! Signature lives in a sibling file at `<endpoint>/manifest.json.sig`
//! (base64url-encoded Ed25519 signature over the canonical manifest bytes).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::debug_log;

/// How often we check for updates. The cache file's `last_check` field is
/// compared against now; if more than TTL has passed, we re-check. 24h is
/// a friendly default — frequent enough to keep users current, rare enough
/// to be invisible.
pub const UPDATE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Hard cap on the manifest body size. The actual manifest is ~1 KB; we
/// allow 32 KB for future-proofing without enabling DoS of the user's
/// memory if the endpoint serves a junk response.
const MANIFEST_MAX_BYTES: usize = 32 * 1024;

/// Hard cap on a single binary download. The largest tokenade binary today
/// is ~50 MB stripped; 256 MB is a generous ceiling. Anything larger is
/// almost certainly a misconfigured server.
const BINARY_MAX_BYTES: u64 = 256 * 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────
// Serialised types — must match release-r2.ts manifest emitter exactly.
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestV1 {
    /// Always 1 for now. Bump when the schema changes incompatibly.
    pub v: u32,
    /// Semver of the release this manifest describes (e.g. "0.3.0").
    pub version: String,
    /// Unix-seconds release timestamp. Informational.
    #[serde(rename = "releasedAt")]
    pub released_at: u64,
    /// Older versions that should NOT auto-update across this boundary
    /// (forces user to read the release notes). Optional.
    #[serde(
        rename = "minSupportedVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub min_supported_version: Option<String>,
    /// One entry per Rust target triple. Missing triple = no binary
    /// published for this platform → updater is a no-op.
    pub targets: std::collections::BTreeMap<String, ManifestTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestTarget {
    /// Absolute HTTPS URL of the binary tarball.
    pub url: String,
    /// Lowercase hex-encoded SHA-256 of the tarball bytes.
    pub sha256: String,
    /// Size of the tarball in bytes — checked at download time as a
    /// cheap first-pass tamper check.
    pub size: u64,
}

mod decide;
mod state;
mod swap;
pub use decide::*;
pub use state::*;
pub(crate) use swap::*;

/// Format the "no binary published for target X" error so the user
/// can debug it: lists all targets the manifest DID publish, or flags
/// a likely server-side mis-publish when the targets map is empty.
/// Shared by all 3 check sites (`check_and_swap_now`, `check_only`,
/// `check_and_swap`) so the error wording stays consistent regardless
/// of how the upgrade was triggered — the background path used to
/// inline a stale terse format until #995 collapsed it back.
fn no_target_msg(manifest: &ManifestV1, target: &str) -> String {
    let mut available: Vec<&str> = manifest.targets.keys().map(|s| s.as_str()).collect();
    available.sort();
    if available.is_empty() {
        format!("no binary published for target {target}; manifest contains 0 targets — likely server-side mis-publish")
    } else {
        format!(
            "no binary published for target {target}; manifest has: {}",
            available.join(", ")
        )
    }
}

/// Reject any manifest the client doesn't know how to handle. Today
/// the only check is the schema version; having a single helper means
/// a future client that wants to ALSO validate (say) a required field
/// will only need to update one site. Pre-#1023 the `v != 1` check
/// lived inline in `check_and_swap_now` + `check_and_swap` but NOT in
/// `check_only`, so `tokenade upgrade --check` would happily surface a
/// v=2 manifest as "available" and then `tokenade upgrade` would
/// refuse it — silent --check vs --swap disagreement.
fn validate_manifest_basics(manifest: &ManifestV1) -> Result<(), String> {
    if manifest.v != 1 {
        return Err(format!(
            "unsupported manifest version v={}, only v=1 is known to this client",
            manifest.v
        ));
    }
    Ok(())
}

/// Returns the compile-time target triple this binary was built for.
/// Used to pick the matching manifest entry.
pub fn current_target_triple() -> &'static str {
    // `target` is a built-in env var Cargo exposes to all crates. We
    // export it via a build.rs-free trick: env! at compile time.
    // For host-build the canonical triple is what we want.
    env!("TOKENADE_TARGET_TRIPLE")
}
// ─────────────────────────────────────────────────────────────────────────
#[cfg(test)]
include!("updater_tests.rs");
