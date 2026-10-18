//! On-demand fetch of the embedding model (see `MODEL_VERSION`).
//!
//! ## Why this exists
//!
//! The model weights (`model.safetensors`, 62 MB) used to be baked into
//! every binary via `include_bytes!` (see `embedding.rs`). That made the
//! release artifacts ~104 MB each and meant the high-entropy blob shipped
//! to every user whether or not they ever called `semantic_search`.
//!
//! The model is immutable per version, so we externalise it: the binary
//! ships ~40 MB, and the first time the embedder is actually needed we
//! download the three model files, extract them into the user cache, and
//! load from there forever after. Hugging Face is the PRIMARY source (keeps
//! the bulk of model traffic off the owner's bucket) with fail-closed
//! baked-sha256 integrity at a pinned revision; the Ed25519-signed R2 tarball
//! is the FALLBACK. See [`download_into`].
//!
//! ## Resolution order (first hit wins)
//!
//! 1. `TOKENADE_MODEL_DIR` — explicit override pointing at a directory that
//!    already holds the three model files. Used by CI / air-gapped installs
//!    and by anyone who wants to pre-seed the model.
//! 2. The user cache (`<cache>/tokenade/model/potion-code-16M-v1/`). If the
//!    three files + an `.ok` marker are present, use them; otherwise
//!    download from Hugging Face (primary) or R2 (fallback), verify, and
//!    extract into it.
//!
//! Network is touched ONLY in case 2 and ONLY on the first call that needs
//! it. Offline-with-no-cache surfaces a clear, actionable error.

use std::fs;
use std::path::{Path, PathBuf};

/// Bumped whenever the model bytes change. The R2 object name, the cache
/// subdir, and the auto-update cadence all key off this — a new model is a
/// new object, so a stale cache is never silently reused.
pub const MODEL_VERSION: &str = "potion-multilingual-128M-i8-v1";

/// The files `crate::static_embed::StaticEmbedder::load` mmaps. Names match the
/// tar entries and the in-repo asset layout.
///
/// `tokenizer.json` is deliberately NOT here: parsing it is what cost 541 MB of
/// RSS, and `vocab.fst` + `vocab.fst.scores` + `vocab.fst.norm` are its baked,
/// mmap-able equivalent. It stays published upstream for re-derivation, but the
/// runtime never reads it.
pub(crate) const MODEL_FILES: [&str; 5] = [
    "model.safetensors",
    "vocab.fst",
    "vocab.fst.scores",
    "vocab.fst.norm",
    "config.json",
];

/// Hugging Face PRIMARY source. The three files are pulled straight from HF
/// (see [`download_from_hf`]); the Ed25519-signed tarball attached to a
/// GitHub Release (see [`download_from_r2`]) is the FALLBACK, reached only when HF can't be —
/// e.g. an HF outage or a sandbox that allow-lists neither host.
///
/// HF is a THIRD PARTY: we cannot Ed25519-verify their bytes. Integrity for
/// this — the primary path — rests entirely on two things:
///   1. [`MODEL_FILE_SHA256`] — a per-file digest baked into the binary,
///      re-checked after download (fail closed on any mismatch).
///   2. [`MODEL_HF_REVISION`] — a pinned *commit* SHA (never `main`), so the
///      served bytes are immutable and can't be swapped under us.
/// The three digests below were confirmed byte-identical to the signed R2
/// model at exactly this revision before being baked.
const MODEL_HF_REPO: &str = "Tokenade/potion-multilingual-128M-i8-tokenade";

/// Pinned commit SHA of [`MODEL_HF_REPO`]. Bumping this is a security-relevant
/// change: re-confirm all three files still match [`MODEL_FILE_SHA256`] at the
/// new revision before touching it.
const MODEL_HF_REVISION: &str = "ba8c55049669aa4c81211002604fac3afc3a7a89";

/// sha256 (hex) of each model file, baked into the binary. Looked up by name,
/// so order is irrelevant. These gate the HF fallback: a downloaded file whose
/// digest is not found here, or does not match, is rejected and nothing is
/// written into the cache.
const MODEL_FILE_SHA256: [(&str, &str); 5] = [
    (
        "model.safetensors",
        "98587b62d9c4040e76b5c5f51d57e69c80b12b78d46fbb6b04f85ddb9b4f5890",
    ),
    (
        "vocab.fst",
        "9ed5ed03bd5082e43cf8a554ccbd9ee8a8acee016e4ba02f1369f35865e10695",
    ),
    (
        "vocab.fst.scores",
        "38e46300114e6fca0b9f630e7c47ab7940a0567e5df0beb5def8fc7d98bce079",
    ),
    (
        "vocab.fst.norm",
        "9226ae18ed464a958157665179c94a8e95e7eb92a8473e79ab6c273a65ee6db5",
    ),
    (
        "config.json",
        "595e4cab2093732efd5dbe084fd5c1826b5eea693b73b4c1fd971672867d2e54",
    ),
];

/// Per-file ceiling for the HF fallback. The safetensors is ~64 MB; the two
/// JSON files are tiny but share this generous cap. Anything larger is a
/// misconfigured mirror and is truncated by `http_get` (→ sha256 mismatch →
/// fail closed).
const MODEL_HF_FILE_MAX_BYTES: usize = 128 * 1024 * 1024;

/// Generous ceiling for the gzipped model tarball (the real artifact is
/// ~58 MB compressed). Anything larger is a misconfigured server.
const MODEL_TARBALL_MAX_BYTES: usize = 128 * 1024 * 1024;
/// Ceiling for the gunzipped tar stream.
const MODEL_RAW_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The signature file is base64url text — a handful of bytes.
const SIG_MAX_BYTES: usize = 4 * 1024;

/// Resolve the model directory, downloading on first use if needed.
///
/// Returns a PATH, not bytes: `StaticEmbedder` mmaps these files, and reading
/// them into a `Vec` first would reintroduce the ~128 MB of resident memory the
/// whole design exists to avoid.
pub fn model_dir() -> Result<PathBuf, String> {
    ensure_model_dir()
}

/// Resolve the model dir **without touching the network**: explicit
/// override (1), in-repo debug asset (2), or an already-populated user
/// cache (3-cache). Returns `None` when only a download would satisfy the
/// request. This is the hermetic half of [`ensure_model_dir`] — tests use
/// it to skip cleanly when no local model is present instead of firing a
/// download at `cargo test` time (which fails closed on offline / fresh CI
/// VMs and is never something a unit test should do).
pub fn offline_model_dir() -> Option<PathBuf> {
    // 1. Explicit override.
    if let Some(dir) = std::env::var_os("TOKENADE_MODEL_DIR") {
        let dir = PathBuf::from(dir);
        if dir_is_complete(&dir) {
            return Some(dir);
        }
    }

    // NO in-repo asset fallback. It used to point at
    // `assets/model2vec/potion-code-16M` — the OLD model — while
    // `MODEL_VERSION` (and therefore `model_fingerprint`) already said
    // `potion-multilingual-128M-i8-v1`. Embeddings are NOT comparable across
    // models, so a debug build could embed with one model, stamp the vectors
    // with the other's fingerprint, and the cache-invalidation guard could
    // never notice — the fingerprint it compares was the lie. A 62 MB vendored
    // copy of the wrong model is worse than no fallback: use
    // `TOKENADE_MODEL_DIR` to pre-seed, or let the on-demand fetch run.

    // 3-cache. Already-populated user cache.
    if let Ok(cache) = cache_model_dir() {
        if cache.join(".ok").is_file() && dir_is_complete(&cache) {
            return Some(cache);
        }
    }

    None
}

/// Return a directory guaranteed to contain all three model files,
/// following the resolution order documented at the module level.
fn ensure_model_dir() -> Result<PathBuf, String> {
    if let Some(dir) = offline_model_dir() {
        return Ok(dir);
    }

    // An explicit override that didn't resolve above is a hard error — the
    // user pointed us at a directory that lacks the files; don't silently
    // fall through to a download they didn't ask for.
    if let Some(dir) = std::env::var_os("TOKENADE_MODEL_DIR") {
        return Err(format!(
            "TOKENADE_MODEL_DIR={} does not contain the model files ({})",
            PathBuf::from(&dir).display(),
            MODEL_FILES.join(", ")
        ));
    }

    // Nothing local — download into the user cache (the only path that
    // touches the network, and only on first use).
    let cache = cache_model_dir()?;

    // Say it BEFORE the wait, not after. The model is ~132 MB and the fetch is
    // synchronous, so the first `--prompt` on a machine blocks for the whole
    // download with nothing on either stream. Measured 2026-08-26: an e2e case
    // sat at 173 seconds and read as a hang — that is what a caller sees, and
    // an agent whose tool call goes quiet for minutes kills it and retries,
    // which starts the download again.
    //
    // Placed HERE, at the single decision point, rather than inside
    // `download_from_hf` and `download_from_r2` — the fallback path would
    // otherwise announce itself twice or not at all, depending on which copy
    // someone remembered to update.
    //
    // stderr, so stdout stays byte-identical: same posture as every other
    // advisory in this codebase.
    eprintln!(
        "[tokenade] first retrieval on this machine: downloading the embedding model \
         (~132 MB, once). Later questions answer from the cache."
    );
    download_into(&cache)?;
    Ok(cache)
}

fn cache_model_dir() -> Result<PathBuf, String> {
    let base = crate::platform::cache_dir().ok_or_else(|| {
        "no cache directory available (set XDG_CACHE_HOME, HOME, or TOKENADE_MODEL_DIR)".to_string()
    })?;
    Ok(base.join("tokenade").join("model").join(MODEL_VERSION))
}

fn dir_is_complete(dir: &Path) -> bool {
    MODEL_FILES.iter().all(|f| dir.join(f).is_file())
}

/// Populate `dest` with the three model files, trying Hugging Face first and
/// falling back to the signed R2 tarball when HF is unreachable.
///
/// Order is deliberate: HF is the PRIMARY source. Its integrity is
/// fail-closed on the per-file sha256 baked into the binary, served from a
/// pinned immutable revision — see [`download_from_hf`]. The GitHub Release
/// tarball is the FALLBACK (Ed25519-signed with the release key), reached only when HF can't be — e.g. a
/// sandbox that allow-lists neither, or an HF outage. Set
/// `TOKENADE_MODEL_NO_HF=1` to skip HF entirely and use R2 only.
fn download_into(dest: &Path) -> Result<(), String> {
    // Escape hatch: force R2-only (HF is otherwise primary).
    if env_is_set("TOKENADE_MODEL_NO_HF") {
        return download_from_r2(dest).map_err(|e| {
            format!(
                "download model from R2 failed ({e}); \
                 Hugging Face primary disabled via TOKENADE_MODEL_NO_HF"
            )
        });
    }

    let hf_err = match download_from_hf(dest) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };

    // Primary (HF) failed. Record it so the fallback attempt (or the combined
    // error below) is diagnosable even when only R2 ends up succeeding.
    crate::debug_log::log(
        "model_fetch",
        "hf_failed",
        serde_json::json!({ "error": hf_err }),
    );

    match download_from_r2(dest) {
        Ok(()) => Ok(()),
        Err(r2_err) => Err(format!(
            "download model failed from both Hugging Face ({hf_err}) \
             and R2 ({r2_err})"
        )),
    }
}

/// `model-<MODEL_VERSION>.tar.gz` (+ `.sig`): attached to the dedicated GitHub
/// Release tagged `model-<MODEL_VERSION>`, or served at the root of a custom
/// `TOKENADE_UPDATE_ENDPOINT` (mirror / test server).
pub(crate) fn model_tarball_url() -> String {
    let name = format!("model-{MODEL_VERSION}.tar.gz");
    let endpoint = crate::env_compat::update_endpoint();
    if endpoint == crate::env_compat::DEFAULT_UPDATE_ENDPOINT {
        format!(
            "{}/download/model-{MODEL_VERSION}/{name}",
            crate::env_compat::GITHUB_RELEASES
        )
    } else {
        format!("{endpoint}/{name}")
    }
}

/// Download the signed model tarball (GitHub Release fallback), verify its Ed25519 signature,
/// and promote the three files into `dest`. This is the FALLBACK path (HF is
/// primary) — bytes are our own, signed with the release-manifest key.
fn download_from_r2(dest: &Path) -> Result<(), String> {
    let base = model_tarball_url();
    let sig_url = format!("{base}.sig");

    crate::debug_log::log(
        "model_fetch",
        "r2_fallback",
        serde_json::json!({ "url": base }),
    );

    let tarball = crate::net::get_body_capped(&base, MODEL_TARBALL_MAX_BYTES)
        .map_err(|e| format!("download model: {e}"))?;
    let sig = crate::net::get_body_capped(&sig_url, SIG_MAX_BYTES)
        .map_err(|e| format!("download model signature: {e}"))?;

    // Same Ed25519 key as the release manifest — fail closed on tamper.
    crate::signing::verify_detached(&tarball, &sig)
        .map_err(|e| format!("model signature verification failed: {e}"))?;

    let raw = gunzip(&tarball)?;
    let entries = read_tar_entries(&raw)?;

    promote_files(dest, &entries)
}

/// Download the three model files directly from Hugging Face at a PINNED
/// revision, verify each against the sha256 baked into the binary, and
/// promote them into `dest`. This is the PRIMARY path (R2 is the fallback —
/// see [`download_into`]), so this sha256 gate is the main integrity check.
///
/// Integrity: HF bytes are not Ed25519-signed, so every file is checked
/// against [`MODEL_FILE_SHA256`] before anything is written — a mismatch (or
/// a name with no baked digest) is a hard error and leaves `dest` untouched.
/// The pinned [`MODEL_HF_REVISION`] makes the served bytes immutable. The
/// `TOKENADE_MODEL_HF_BASE` env overrides the host (default
/// `https://huggingface.co`) for mirrors / offline testing.
fn download_from_hf(dest: &Path) -> Result<(), String> {
    let base = std::env::var("TOKENADE_MODEL_HF_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://huggingface.co".to_string());
    let base = base.trim_end_matches('/');

    crate::debug_log::log(
        "model_fetch",
        "hf_primary",
        serde_json::json!({ "repo": MODEL_HF_REPO, "revision": MODEL_HF_REVISION }),
    );

    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(MODEL_FILES.len());
    for name in MODEL_FILES {
        // A file with no baked digest must never be installed — fail closed.
        let want = MODEL_FILE_SHA256
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| *s)
            .ok_or_else(|| format!("no baked sha256 for {name}; refusing HF fallback"))?;

        let url = format!("{base}/{MODEL_HF_REPO}/resolve/{MODEL_HF_REVISION}/{name}");
        let bytes = crate::net::get_body_capped(&url, MODEL_HF_FILE_MAX_BYTES)
            .map_err(|e| format!("download {name} from Hugging Face: {e}"))?;

        let got = sha256_hex(&bytes);
        if got != want {
            return Err(format!(
                "Hugging Face {name} sha256 mismatch: got {got}, want {want} \
                 (revision {MODEL_HF_REVISION})"
            ));
        }
        files.push((name.to_string(), bytes));
    }

    promote_files(dest, &files)
}

/// Atomically install the model into `dest`: write the three [`MODEL_FILES`]
/// (looked up by name in `files`) plus an `.ok` marker into a sibling temp
/// dir, then rename it into place. A half-finished download is therefore
/// never mistaken for a good cache. Shared by the R2 and HF paths so both
/// promote bytes identically; any missing file name is a hard error.
fn promote_files(dest: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let parent = dest
        .parent()
        .ok_or_else(|| format!("model cache dir has no parent: {}", dest.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;

    // The staging dir must be unique per CALL, not per process. Keyed on the pid
    // alone, two concurrent promotes inside one process — different `dest`, same
    // `parent` — resolve to the SAME path, and each one starts by wiping it:
    // `remove_dir_all(&tmp)` deletes the other's staged files mid-flight, so a
    // promote that reported success left `dest` incomplete. This is what made
    // `promote_files_installs_all_three_plus_ok` fail intermittently whenever
    // the suite ran alongside another crate's tests (the two promote tests share
    // `temp_dir()` as parent and run on parallel threads of one binary).
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = parent.join(format!(".{MODEL_VERSION}.tmp.{}.{seq}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;

    for name in MODEL_FILES {
        let bytes = files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b)
            .ok_or_else(|| format!("model download missing {name}"))?;
        fs::write(tmp.join(name), bytes).map_err(|e| format!("write {name}: {e}"))?;
    }
    fs::write(tmp.join(".ok"), b"").map_err(|e| format!("write .ok marker: {e}"))?;

    // Atomic-ish promote: remove any stale dest, then rename temp → dest.
    let _ = fs::remove_dir_all(dest);
    fs::rename(&tmp, dest).map_err(|e| {
        let _ = fs::remove_dir_all(&tmp);
        format!(
            "promote model cache {} → {}: {e}",
            tmp.display(),
            dest.display()
        )
    })?;
    crate::debug_log::log(
        "model_fetch",
        "cached",
        serde_json::json!({ "dir": dest.display().to_string() }),
    );
    Ok(())
}

/// Lowercase hex sha256 of `bytes`, matching the digest format baked into
/// [`MODEL_FILE_SHA256`] and used by the updater's manifest verification.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// True when `var` is present in the environment and set to a truthy value
/// (anything other than empty / `0` / `false`). Used for the opt-out flags.
fn env_is_set(var: &str) -> bool {
    match std::env::var(var) {
        Ok(v) => {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        }
        Err(_) => false,
    }
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut out = Vec::with_capacity(bytes.len() * 2);
    GzDecoder::new(bytes)
        .take(MODEL_RAW_MAX_BYTES)
        .read_to_end(&mut out)
        .map_err(|e| format!("gunzip model: {e}"))?;
    Ok(out)
}

/// Minimal POSIX tar reader returning `(name, bytes)` for every regular
/// file entry. Unlike the updater's single-entry reader, the model tarball
/// carries three files, so we collect them all and match by name. Handles
/// GNU long-name (`L`) and PAX (`x`) extension records by ignoring them and
/// falling back to the 100-byte header name field, which is enough for our
/// short, fixed filenames.
fn read_tar_entries(raw: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    const BLOCK: usize = 512;
    let mut out = Vec::new();
    let mut off = 0;
    while off + BLOCK <= raw.len() {
        let hdr = &raw[off..off + BLOCK];
        if hdr.iter().all(|&b| b == 0) {
            break; // end-of-archive marker
        }
        // Name: bytes 0..100, NUL-terminated. We only need the basename.
        let name_raw = &hdr[0..100];
        let name_end = name_raw.iter().position(|&b| b == 0).unwrap_or(100);
        let name = String::from_utf8_lossy(&name_raw[..name_end]).to_string();
        let basename = name.rsplit('/').next().unwrap_or(&name).to_string();

        // Size: bytes 124..136, NUL/space-padded octal.
        let size_field =
            std::str::from_utf8(&hdr[124..124 + 12]).map_err(|_| "tar size field not utf-8")?;
        let cleaned = size_field.trim_matches(|c: char| c == '\0' || c.is_ascii_whitespace());
        let size = if cleaned.is_empty() {
            0
        } else {
            u64::from_str_radix(cleaned, 8).map_err(|e| format!("tar size parse: {e}"))?
        };

        let typeflag = hdr[156];
        let data_start = off + BLOCK;
        let data_end = data_start + size as usize;
        if data_end > raw.len() {
            return Err("tar: truncated data block".into());
        }
        if typeflag == 0 || typeflag == b'0' {
            out.push((basename, raw[data_start..data_end].to_vec()));
        }
        let blocks = size.div_ceil(BLOCK as u64) as usize;
        off += BLOCK + blocks * BLOCK;
    }
    if out.is_empty() {
        return Err("tar: no regular file entries".into());
    }
    Ok(out)
}
#[cfg(test)]
include!("model_fetch_tests.rs");
