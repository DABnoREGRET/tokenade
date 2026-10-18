//! Signed savings receipt — a tamper-evident, **fully local** proof of how
//! many tokens tokenade has saved on this machine.
//!
//! ## What it is
//!
//! The receipt aggregates the local `gain.jsonl` ledger (the same one the
//! dashboard reads) into a small body — events, tokens before/after, tokens
//! saved — and signs it with a per-install Ed25519 key. The output carries
//! the public key and the signature, so anyone can verify the body was not
//! edited after it was generated (`tokenade receipt --verify <file>`).
//!
//! ## Why a *self-attested* key
//!
//! A savings receipt has to be generated offline, so it is signed with a keypair generated ON this
//! install and stored next to the ledger (0600). That makes the receipt
//! tamper-EVIDENT (the body can't be altered without breaking the signature)
//! and ties it to a stable install identity — it is NOT a claim that
//! Anthropic or the tokenade project vouches for the number. Nothing is sent anywhere:
//! generating a receipt touches only local files, honouring the
//! "stats stay 100% local" rule. The number is exactly what the local ledger
//! already shows; the signature just makes it portable and hard to fudge.
//!
//! Reuses the existing `ed25519-dalek` dependency (v2 API) and the
//! `gain` ledger plumbing — no new crates.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Receipt schema marker. Bumped to v2 on 2026-08-10 when
/// `tokens_saved_approximated` entered the SIGNED body: the field changes the
/// canonical bytes, so a v1 receipt cannot be verified by a v2 reader and must
/// be refused by kind rather than by a misleading signature failure.
const RECEIPT_KIND: &str = "tokenade.savings-receipt.v2";

/// File name of the per-install signing seed, stored beside the ledger.
const KEY_FILE: &str = "receipt-ed25519.key";

/// The signed payload. Field order here IS the canonical signing order —
/// `serde_json::to_vec` serialises struct fields in declaration order, so the
/// bytes are deterministic across runs and machines. Everything is an integer
/// or a short string (no floats) so there is no formatting ambiguity to sign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptBody {
    /// Receipt schema marker (lets a verifier reject a future incompatible body).
    pub kind: String,
    /// tokenade version that produced the receipt.
    pub version: String,
    /// Stable per-install id: a hash of this install's receipt public key —
    /// not a machine fingerprint, not PII.
    pub machine: String,
    /// When the receipt was generated (unix seconds).
    pub generated_at: u64,
    /// Number of ledger events folded into the totals.
    pub events: u64,
    /// Tokens that would have been spent without tokenade.
    pub tokens_before: u64,
    /// Tokens actually spent after compaction.
    pub tokens_after: u64,
    /// `tokens_before - tokens_after` (stored explicitly so the headline
    /// number is part of what's signed, not recomputed by a reader).
    pub tokens_saved: u64,
    /// How much of `tokens_saved` rests on an APPROXIMATION.
    ///
    /// Some channels REFUSE an oversized payload instead of truncating it: the
    /// agent delivers nothing and the user re-reads the file in slices, which
    /// costs real tokens that no ledger row can reconstruct. Crediting the cap
    /// there is a deliberate stand-in (decision, 2026-08-10) — closer to the
    /// truth than zero, and not a measurement. It is SIGNED rather than printed
    /// as a footnote because a caveat that can be dropped in transit is not a
    /// caveat: the number and the fact that part of it is estimated have to
    /// travel together, or the receipt overstates itself the moment someone
    /// quotes it.
    #[serde(default)]
    pub tokens_saved_approximated: u64,
}

impl ReceiptBody {
    /// Saved fraction as a percentage, computed on demand (kept OUT of the
    /// signed body to avoid float-formatting ambiguity).
    pub fn percent(&self) -> f64 {
        if self.tokens_before == 0 {
            return 0.0;
        }
        self.tokens_saved as f64 / self.tokens_before as f64 * 100.0
    }

    /// Canonical bytes that get signed / verified.
    fn signing_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// A receipt body plus the public key and signature proving its integrity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedReceipt {
    pub body: ReceiptBody,
    /// 64-hex of the 32-byte Ed25519 verifying key.
    pub pubkey: String,
    /// 128-hex of the 64-byte signature over `body`'s canonical bytes.
    pub signature: String,
}

/// Directory holding local tokenade data (the ledger). Beside it we keep the
/// receipt signing key. Routing through `gain::current_ledger_path()` means a
/// test that calls `gain::set_root` redirects the key too — hermetic tests,
/// no writes to the real `~/.tokenade`.
fn data_dir() -> PathBuf {
    crate::gain::current_ledger_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn key_path() -> PathBuf {
    data_dir().join(KEY_FILE)
}

/// Fill `buf` with OS randomness. Unix reads `/dev/urandom` (real entropy);
/// the non-unix fallback mixes time/pid/machine-id, which is weaker but
/// acceptable here — the receipt is self-attested, so an attacker who can
/// run on this machine already holds the private key (no `rand` dependency).
fn fill_random(buf: &mut [u8]) {
    #[cfg(unix)]
    {
        // Read EXACTLY buf.len() bytes — `/dev/urandom` is an infinite stream,
        // so `fs::read` (read-to-EOF) would allocate without bound. Use a
        // bounded `read_exact` instead.
        use std::io::Read as _;
        if let Ok(mut f) = fs::File::open("/dev/urandom") {
            if f.read_exact(buf).is_ok() {
                return;
            }
        }
    }
    // Fallback: derive from a hash of volatile inputs.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(crate::platform::now_unix_secs().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update((buf.as_ptr() as usize).to_le_bytes());
    let mut seed = hasher.finalize().to_vec();
    // Stretch to cover buf if needed.
    while seed.len() < buf.len() {
        let mut h = Sha256::new();
        h.update(&seed);
        seed.extend_from_slice(&h.finalize());
    }
    buf.copy_from_slice(&seed[..buf.len()]);
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("hex length not even".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("bad hex: {e}")))
        .collect()
}

/// Load the per-install signing key, generating and persisting one (0600) on
/// first use. The file holds the raw 32-byte Ed25519 seed.
fn load_or_create_key() -> std::io::Result<SigningKey> {
    let path = key_path();
    if let Ok(bytes) = fs::read(&path) {
        if bytes.len() == 32 {
            let seed: [u8; 32] = bytes.as_slice().try_into().expect("len checked == 32");
            return Ok(SigningKey::from_bytes(&seed));
        }
        // Wrong size → treat as corrupt and regenerate below.
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut seed = [0u8; 32];
    fill_random(&mut seed);
    let key = SigningKey::from_bytes(&seed);

    // Write 0600 on unix.
    let mut opts = fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write as _;
    let mut f = opts.open(&path)?;
    f.write_all(&seed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}

/// Aggregate the local gain ledger into a totals body. Reads only the
/// non-inflating gain ledger (never the `bypass_lost.jsonl`), so the headline
/// number can never exceed what was genuinely saved.
///
/// **Shares `gain::scan_ledger` with the other four surfaces, since 2026-08-10.**
/// It used to run its OWN loop over `gain.jsonl` alone — a fifth copy of the
/// savings computation, and the only one that ignored the rollup. So every
/// rotation moved history out of the signed receipt and into nothing: the
/// dashboard, `gain`, the statusline and the terminal all counted it, the
/// receipt did not. Measured on the shared fixture: 5 400 against 14 400, the
/// difference being exactly the rolled-up rows.
///
/// The receipt is the SIGNED figure — the one a user quotes to say what
/// tokenade did — so it is the worst of the five to have disagreeing. Pinned by
/// `every_surface_reports_the_same_measured_savings`, which now covers all five.
pub fn aggregate() -> ReceiptBody {
    let path = crate::gain::current_ledger_path();
    let content = crate::gain::ledger_with_rollup(&path);
    // Window 0 = lifetime. A receipt states what was saved, not what was saved
    // recently, and a windowed figure in a signed document would be a claim
    // whose meaning changes with the date it is read.
    let scan = crate::gain::scan_ledger(&content, 0);
    let (events, before, after) = (scan.calls, scan.measured_before, scan.measured_after);
    ReceiptBody {
        kind: RECEIPT_KIND.to_string(),
        version: crate::VERSION.to_string(),
        machine: String::new(), // filled by `build` from the install key
        generated_at: crate::platform::now_unix_secs(),
        events,
        tokens_before: before,
        tokens_after: after,
        tokens_saved: before.saturating_sub(after),
        tokens_saved_approximated: scan.approximated_saved,
    }
}

/// Build and sign a receipt over the current local ledger.
pub fn build() -> std::io::Result<SignedReceipt> {
    let mut body = aggregate();
    let key = load_or_create_key()?;
    body.machine = install_id(&key);
    let sig = key.sign(&body.signing_bytes());
    Ok(SignedReceipt {
        pubkey: to_hex(key.verifying_key().as_bytes()),
        signature: to_hex(&sig.to_bytes()),
        body,
    })
}

/// Stable per-install id: first 16 bytes of SHA-256 over the receipt public key.
fn install_id(key: &SigningKey) -> String {
    use sha2::{Digest, Sha256};
    to_hex(&Sha256::digest(key.verifying_key().as_bytes())[..16])
}

/// Verify a receipt's signature against the public key it carries. Returns
/// `Ok(())` when the body is intact, `Err(reason)` otherwise. This proves the
/// body was not edited after signing — it does NOT prove which install signed
/// it (the key is self-attested by design).
pub fn verify(r: &SignedReceipt) -> Result<(), String> {
    // v1 is NOT accepted here, and the reason is worth stating. A v1 body has
    // no `tokens_saved_approximated`; parsing it defaults the field to 0 and
    // re-serialising it ADDS the field, so the bytes no longer match what was
    // signed and the check would fail with "signature verify failed" — a
    // message that accuses the receipt of being tampered with when the only
    // thing that changed is our schema. Rejecting by KIND says the true thing.
    if r.body.kind != RECEIPT_KIND {
        return Err(format!(
            "unknown receipt kind: {} (this build reads {RECEIPT_KIND})",
            r.body.kind
        ));
    }
    let pk = from_hex(&r.pubkey)?;
    let pk: [u8; 32] = pk
        .as_slice()
        .try_into()
        .map_err(|_| format!("pubkey must be 32 bytes, got {}", r.pubkey.len() / 2))?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|e| format!("bad pubkey: {e}"))?;
    let sig = from_hex(&r.signature)?;
    let sig: [u8; 64] = sig
        .as_slice()
        .try_into()
        .map_err(|_| format!("signature must be 64 bytes, got {}", r.signature.len() / 2))?;
    let sig = Signature::from_bytes(&sig);
    vk.verify(&r.body.signing_bytes(), &sig)
        .map_err(|e| format!("signature verify failed: {e}"))
}

/// Pretty single-receipt rendering for the CLI.
/// The caveat line, printed only when there IS something to caveat.
///
/// A permanent disclaimer on every receipt would be read as boilerplate and
/// stop being read at all; one that appears exactly when part of the figure is
/// a stand-in still carries information.
fn approximation_line(b: &ReceiptBody) -> String {
    if b.tokens_saved_approximated == 0 {
        return String::new();
    }
    let pct = if b.tokens_saved == 0 {
        0.0
    } else {
        b.tokens_saved_approximated as f64 * 100.0 / b.tokens_saved as f64
    };
    format!(
        "of which     {} tokens ({:.1}%) are APPROXIMATE\n\
         \x20            those payloads would have been REFUSED, not truncated;\n\
         \x20            the credit stands in for the slice-by-slice re-reading\n\
         \x20            that follows, which cannot be measured after the fact\n",
        b.tokens_saved_approximated, pct
    )
}

pub fn render(r: &SignedReceipt) -> String {
    let b = &r.body;
    format!(
        "tokenade savings receipt\n\
         ────────────────────────\n\
         machine     {}\n\
         generated   {} (unix)\n\
         version     {}\n\
         events      {}\n\
         before      {} tokens\n\
         after       {} tokens\n\
         saved       {} tokens ({:.1}%)\n\
         {}\
         pubkey      {}\n\
         signature   {}",
        b.machine,
        b.generated_at,
        b.version,
        b.events,
        b.tokens_before,
        b.tokens_after,
        b.tokens_saved,
        b.percent(),
        approximation_line(b),
        r.pubkey,
        r.signature,
    )
}

/// Serialise a receipt to pretty JSON (the form written to a file / shared).
pub fn to_json(r: &SignedReceipt) -> String {
    serde_json::to_string_pretty(r).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_fixture() -> ReceiptBody {
        ReceiptBody {
            kind: RECEIPT_KIND.to_string(),
            version: "9.9.9".to_string(),
            machine: "deadbeef".to_string(),
            generated_at: 1_700_000_000,
            events: 3,
            tokens_before: 1000,
            tokens_after: 250,
            tokens_saved: 750,
            tokens_saved_approximated: 0,
        }
    }

    #[test]
    fn the_caveat_appears_only_when_part_of_the_figure_is_a_stand_in() {
        let mut b = body_fixture();
        let clean = approximation_line(&b);
        assert!(clean.is_empty(), "nothing approximate, nothing to say");
        b.tokens_saved_approximated = 300; // of 750
        let line = approximation_line(&b);
        assert!(line.contains("300 tokens"), "{line}");
        assert!(
            line.contains("40.0%"),
            "share of the headline, not raw: {line}"
        );
        assert!(line.contains("REFUSED"), "it must say WHY: {line}");
        // Zero headline must not divide by zero into a nonsense percentage.
        b.tokens_saved = 0;
        assert!(approximation_line(&b).contains("0.0%"));
    }

    #[test]
    fn a_v1_receipt_is_refused_by_kind_not_by_a_bogus_signature_failure() {
        // A v1 body lacks the approximation field. Parsing defaults it to 0 and
        // re-serialising ADDS it, so the signature check would fail and blame
        // the receipt for something our own schema change did.
        let mut b = body_fixture();
        b.kind = "tokenade.savings-receipt.v1".to_string();
        let r = SignedReceipt {
            body: b,
            pubkey: "00".repeat(32),
            signature: "00".repeat(64),
        };
        let err = verify(&r).expect_err("v1 must not verify against a v2 reader");
        assert!(err.contains("unknown receipt kind"), "{err}");
        assert!(
            !err.contains("signature verify failed"),
            "the message must not accuse the receipt: {err}"
        );
    }

    #[test]
    fn percent_basic_and_zero_guard() {
        let b = body_fixture();
        assert!((b.percent() - 75.0).abs() < 1e-9);
        let mut z = b;
        z.tokens_before = 0;
        assert_eq!(z.percent(), 0.0);
    }

    #[test]
    fn hex_roundtrips() {
        let bytes = [0u8, 1, 2, 250, 255, 16];
        let h = to_hex(&bytes);
        assert_eq!(h, "000102faff10");
        assert_eq!(from_hex(&h).unwrap(), bytes);
        assert!(from_hex("abc").is_err()); // odd length
        assert!(from_hex("zz").is_err()); // non-hex
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        // Sign a body with a fresh key and confirm it verifies.
        let mut seed = [7u8; 32];
        seed[0] = 1;
        let key = SigningKey::from_bytes(&seed);
        let body = body_fixture();
        let sig = key.sign(&body.signing_bytes());
        let r = SignedReceipt {
            pubkey: to_hex(key.verifying_key().as_bytes()),
            signature: to_hex(&sig.to_bytes()),
            body,
        };
        assert!(verify(&r).is_ok(), "freshly signed receipt must verify");
    }

    #[test]
    fn tampered_body_fails_verification() {
        let seed = [3u8; 32];
        let key = SigningKey::from_bytes(&seed);
        let body = body_fixture();
        let sig = key.sign(&body.signing_bytes());
        let mut r = SignedReceipt {
            pubkey: to_hex(key.verifying_key().as_bytes()),
            signature: to_hex(&sig.to_bytes()),
            body,
        };
        // Inflate the headline number after signing — must be detected.
        r.body.tokens_saved += 1_000_000;
        assert!(verify(&r).is_err(), "tampered body must fail");
    }

    #[test]
    fn wrong_kind_rejected() {
        let seed = [9u8; 32];
        let key = SigningKey::from_bytes(&seed);
        let mut body = body_fixture();
        body.kind = "something.else".to_string();
        let sig = key.sign(&body.signing_bytes());
        let r = SignedReceipt {
            pubkey: to_hex(key.verifying_key().as_bytes()),
            signature: to_hex(&sig.to_bytes()),
            body,
        };
        assert!(verify(&r).is_err(), "unknown kind must be rejected");
    }

    #[test]
    fn build_aggregates_ledger_and_verifies() {
        // Hermetic: redirect the ledger root to a tempdir, write a couple of
        // gain rows, build a receipt and confirm the totals + signature.
        let dir = std::env::temp_dir().join(format!(
            "tokenade-core-receipt-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        crate::gain::set_root(dir.clone());
        let ledger = crate::gain::current_ledger_path();
        if let Some(p) = ledger.parent() {
            let _ = fs::create_dir_all(p);
        }
        let rows = "\
{\"ts\":1,\"op\":\"exec\",\"before\":1000,\"after\":200,\"saved\":800}\n\
{\"ts\":2,\"op\":\"read\",\"before\":500,\"after\":100,\"saved\":400}\n";
        fs::write(&ledger, rows).unwrap();

        let r = build().expect("build receipt");
        assert_eq!(r.body.events, 2);
        assert_eq!(r.body.tokens_before, 1500);
        assert_eq!(r.body.tokens_after, 300);
        assert_eq!(r.body.tokens_saved, 1200);
        assert!(verify(&r).is_ok(), "built receipt must verify");

        // Key persisted with the expected size and reused on a second build.
        assert_eq!(fs::read(key_path()).unwrap().len(), 32);
        let r2 = build().expect("second build");
        assert_eq!(r2.pubkey, r.pubkey, "signing key is stable across builds");

        let _ = fs::remove_dir_all(&dir);
    }
}
