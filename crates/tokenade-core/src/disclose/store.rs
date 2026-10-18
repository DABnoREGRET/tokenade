//! **The stash: write, read back, and make room.**
//!
//! A stashed body that can no longer be reopened is not a fold, it is a
//! loss. The stash therefore carries a reachable hash, a lifetime, and an
//! eviction that never removes the ONLY copy of a content.

use super::*;

/// Write `text` to the stash if the destination doesn't already
/// exist (content-addressable: same hash = same bytes). Idempotent.
/// Returns the final path (regardless of whether the write happened).
pub(super) fn ensure_stashed(text: &str, hash: &str, ext: &str) -> std::io::Result<PathBuf> {
    // Sweep where the growth is. This used to sit at the TOP of `maybe_stash`
    // / `maybe_stash_bytes` / `stash_for_reference`, i.e. BEFORE the size
    // threshold that declines to stash at all — so a three-byte `echo hi` paid
    // a full walk of the stash dir. Here it runs only on the paths that
    // actually add a file, which is the only reason the sweep exists, and it
    // covers every writer including `stash_as_binary`.
    opportunistic_evict();
    let dir = stash_dir();
    std::fs::create_dir_all(&dir)?;
    let path = stash_path(hash, ext);
    note_session_ref(hash);
    // Hard cap on stash size — head + tail bytes only past the cap.
    let content: std::borrow::Cow<'_, str> = if text.len() <= STASH_MAX_BYTES {
        text.into()
    } else {
        let half_cap = STASH_MAX_BYTES / 2;
        let (head, tail) = head_tail_at_cap(text, half_cap);
        format!(
            "{head}\n\
             [tokenade:stash truncated — original was {} bytes, kept first {} + last {}]\n\
             {tail}",
            text.len(),
            head.len(),
            tail.len(),
        )
        .into()
    };
    keep_blob(&path, content.as_bytes())?;
    Ok(path)
}

/// **A blob is promised only if it holds what it is named for.**
///
/// An existing file used to be TOUCHED and returned, whatever it held; and the
/// write itself was `truncate` + `write_all`, in place. A write cut short — a
/// full disk, a killed process — left a partial file under the content's own
/// hash, and the next stash of the same output trusted it: the banner promised
/// `expand-ref <hash>` and the recovery handed back the head of the output as
/// if it were all of it. caveman (2026-10) verifies length and digest before
/// publishing an elision; here the name already IS the digest, so the length
/// is the check, and the write goes through a temporary file and a rename so a
/// partial file never carries the name at all.
pub(super) fn keep_blob(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() == content.len() as u64) {
        // Bump mtime so the TTL eviction sees the touch.
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
        return Ok(());
    }
    crate::platform::write_atomic_600(path, content)
}

/// Once-per-process opportunistic eviction. Wraps [`evict_stale`]
/// behind a [`std::sync::OnceLock`] so concurrent `maybe_stash`
/// calls within the same process share a single sweep. Each Bash
/// hook is a fresh process, so this fires roughly once per
/// stash-triggering shell command — natural cadence without spam.
///
/// Errors are silently swallowed: eviction is best-effort, never
/// blocking. The stash gets bounded eventually; if a single sweep
/// fails (permissions, disk full), the next process retries.
/// How often the opportunistic stash sweep is allowed to walk the stash dir.
/// The TTL it enforces is 7 days ([`CACHE_TTL_SECS`]); sweeping for it more
/// than daily buys nothing and costs a full directory walk each time.
pub(super) const STASH_SWEEP_TTL_SECS: u64 = 24 * 3600;
pub(super) const STASH_SWEEP_STAMP: &str = "stash-ttl-sweep";

pub(super) fn opportunistic_evict() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        // This crate's tests own their stash dir (`isolated_stash`) and
        // exercise `evict_stale` directly, so sweeping from here only reached
        // the developer's REAL stash as a side effect of running the suite.
        // Note the reach of this guard: `cfg!(test)` is FALSE here when another
        // crate's test binary links this one, so it silences the sweep for
        // `tokenade-core`'s tests only — which is where the stash tests live.
        if cfg!(test) {
            return;
        }
        // Once per process was never a cadence: every hook is a fresh process,
        // so this walked the whole stash dir on EVERY wrapped command —
        // measured at 9.9 ms of the 30 ms `tokenade wrap 'echo hi'` spends,
        // for a sweep that finds nothing 99 times out of 100. Stamp FIRST (like
        // `ensure_hooks`) so two sessions opening at once don't both walk it.
        let home = crate::platform::home_dir();
        if let Some(h) = &home {
            if !crate::stores::stamp_due(h, STASH_SWEEP_STAMP, STASH_SWEEP_TTL_SECS) {
                return;
            }
            crate::stores::mark_stamp(h, STASH_SWEEP_STAMP);
        }
        let started = std::time::Instant::now();
        if let Ok(removed) = evict_stale() {
            if removed > 0 {
                // Log only when work was actually done — silence on
                // the common "nothing to evict" case keeps the
                // debug.log signal-to-noise high. Removed > 0 means
                // the stash dir had files older than the 7-day TTL.
                crate::debug_log::log(
                    "disclose",
                    "stash_evicted",
                    serde_json::json!({
                        "removed": removed,
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    }),
                );
            }
        }
    });
}

/// The universal entry point. Three dispositions, picked in order:
///
///   1. **Binary content** (PNG, PDF, archives, ELF, fonts, audio,
///      video, anything `infer` recognises OR with ≥5 % control
///      bytes in the first 4 KB): ALWAYS stash, return
///      metadata-only — bytes are unreadable as text so a snippet
///      would pollute the agent's context with garbage.
///   2. **Text above threshold** (≥ `TOKENADE_DISCLOSE_THRESHOLD`,
///      default 5 000 tokens): stash + structural summary.
///   3. **Text below threshold**: return unchanged.
///
/// Best-effort I/O: any error returns the original text so we never
/// silently drop output. Stash failures degrade to passthrough.
/// The `[stashed: …]` pointer [`maybe_stash`] appends to a body it folded, if
/// this text carries one. One spelling of the marker, so a route that has to
/// find it again cannot drift from the route that writes it.
///
/// A route that REPLACES a stashed body with a NARROWER answer must carry this
/// line over. `tokenade web --prompt "…"` did not: the page was converted,
/// stashed and given its pointer, and then the scoped extract overwrote the
/// whole thing, pointer included. Measured on a 59 552-byte page, the same
/// bytes and the same six questions: `read --prompt` ended on `expand-ref
/// e9afe97305de`, `web --prompt` ended on the last line of the answer, 13 544
/// tokens folded away with nothing naming a way back. Worst on the documented
/// pipe form (`curl -s <url> | tokenade web -`), where the bytes are off the
/// network and gone from the shell — the stash file was written either way and
/// only its name was thrown away.
pub fn stash_pointer_line(text: &str) -> Option<&str> {
    text.lines().rev().find(|l| l.starts_with("[stashed: "))
}
