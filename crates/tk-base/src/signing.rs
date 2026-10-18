//! **A detached signature is verified in one place only.**
//!
//! Two things signed by the same key reach this binary: the update
//! manifest (`<endpoint>/manifest.json` + its neighbouring `.sig`) and the archive
//! of a downloaded model.
//!
//! Verification lived in `updater`, a PRODUCT layer, and `model_fetch`
//! — the code layer — named it by climbing up. A cryptographic
//! primitive does not belong to the module that asked for it first;
//! it is the third case of this refactoring after `archetypes` and `session`.
//!
//! What the shape guarantees, written here because this is the only
//! place that decides it: **the signature is the source of truth, not TLS.**
//! A manifest served in clear and correctly signed is accepted; a
//! manifest served over HTTPS and badly signed is not.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// **Release-signing public key (Ed25519, 32 bytes hex).**
///
/// Verifies the update manifest (`manifest.json` + `manifest.json.sig`) and
/// the model-tarball fallback. Generated for the open-source 2.0 line; the
/// private half lives only in the release pipeline's secret store (GitHub
/// Actions secret). Rotating it = replace this constant and ship a release
/// signed with the OLD key that carries the new binary.
pub const RELEASE_PUBKEY_HEX: &str =
    "088950e7ee18e9968f12276d5d6351ad120c47e95ea63eb4958d3da206f2f31f";

pub fn embedded_pubkey_hex() -> String {
    RELEASE_PUBKEY_HEX.to_string()
}

/// **The public key in force**: the environment's
/// (`TOKENADE_UPDATE_PUBKEY_HEX`, for a test bench signing with its
/// own) if it carries one, otherwise the embedded one.
///
/// `std::env::var` returns `Ok("")` for a variable set EMPTY: the
/// embedded fallback must cover that case too.
pub fn current_pubkey_hex() -> String {
    match std::env::var("TOKENADE_UPDATE_PUBKEY_HEX") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => embedded_pubkey_hex(),
    }
}

/// URL-safe Base64, with or without padding. The signatures emitted by the
/// release pipeline carry none; an operator copying a value by hand
/// may add some.
pub fn base64_url_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let cleaned = s.trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cleaned)
        .map_err(|e| e.to_string())
}

/// Verifies a DETACHED ed25519 signature over `payload`.
///
/// The emitter (release pipeline, `JSON.stringify` with a fixed key order) writes the canonical bytes as is,
/// so we verify the fetched bytes without touching them. Any
/// renormalisation here — reformatting the JSON, re-trimming the body — would break
/// verification without saying anything useful.
///
/// `TOKENADE_UPDATE_PUBKEY_HEX` replaces the embedded key, for a test
/// bench signing with its own.
pub fn verify_detached(payload: &[u8], sig_b64url: &[u8]) -> Result<(), String> {
    let sig_str = std::str::from_utf8(sig_b64url)
        .map_err(|e| format!("signature not utf-8: {e}"))?
        .trim();
    let sig_bytes =
        base64_url_decode(sig_str).map_err(|e| format!("signature base64 decode: {e}"))?;
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("signature wrong length: {} (want 64)", sig_bytes.len()))?;
    let sig = Signature::from_bytes(&sig_arr);

    let pub_hex = current_pubkey_hex();
    let pub_bytes = hex::decode(pub_hex.trim()).map_err(|e| format!("pubkey hex: {e}"))?;
    let pub_arr: [u8; 32] = pub_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "pubkey wrong length")?;
    let vk = VerifyingKey::from_bytes(&pub_arr).map_err(|e| format!("pubkey parse: {e}"))?;
    vk.verify(payload, &sig)
        .map_err(|e| format!("ed25519 verify: {e}"))
}
