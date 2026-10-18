//! A page fetched twice in one session, delivered once.
//!
//! Measured on a real ledger: **13.8 % of everything the web path delivers is a
//! re-delivery of a page already delivered in the same session** — the same URL
//! fetched three or four times, ~29 k tokens each time, while the first copy was
//! still sitting in the agent's context. That is the largest single item left on
//! the biggest surface this product has, and it is not a compaction problem: the
//! text was already correct, it was simply sent again.
//!
//! ## Why this is lossless, and how it differs from the command replay
//!
//! `hooks::replay` answers "may I skip running this command?" and has to prove
//! the world did not move — a filesystem fingerprint, a write generation. A page
//! is remote and we cannot fingerprint it, so this does the opposite and cheaper
//! thing: **fetch anyway, then compare what we would deliver against what we
//! already delivered.** Identical bytes and only identical bytes are folded, so
//! a page that changed between two fetches is delivered in full, every time.
//! The saving is the second copy of something the agent can already see.
//!
//! ## What still has to hold
//!
//! Two guards, both borrowed from the command path because they answer the same
//! question — *is the first copy still in the model's context?*
//!
//!   * freshness: past the window, the first copy has probably scrolled out of
//!     an agent's working set, and "see above" would point at nothing;
//!   * no compaction since: a summary squashed the transcript, so the first copy
//!     is provably gone.
//!
//! `TOKENADE_NO_REPLAY=1` disables it, same switch as the command path.

use std::path::PathBuf;

/// How long a delivered page stays replayable. Past five minutes an agent's
/// first copy is usually out of reach.
///
/// **The ONLY reader of `TOKENADE_REPLAY_MAX_AGE_SECS`.** The command route
/// carried a word-for-word copy, default included; two readers of the same
/// variable end up no longer saying the same thing, and it happened that very
/// day to another environment variable.
pub fn max_age_secs() -> u64 {
    std::env::var("TOKENADE_REPLAY_MAX_AGE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

/// Below this, saying "you already have it" costs more than saying it again.
const MIN_TOKENS_TO_FOLD: usize = 400;

/// The threshold, for whoever must avoid pushing a page below it.
///
/// A fold applied upstream can make a page short enough for this mechanism to
/// stop recognising it: the page then goes out WHOLE on the second call, and
/// the fold cost more than it saved. Same rule as for the stash threshold, on
/// one more threshold.
pub fn min_tokens_to_fold() -> usize {
    MIN_TOKENS_TO_FOLD
}

/// Where the per-session records live. The pure core below takes this as a
/// parameter: a test that mutates `HOME` passes alone and lies in company, and
/// this suite runs in parallel.
fn sessions_dir() -> PathBuf {
    crate::platform::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
}

fn state_path(dir: &std::path::Path, session: &str, url_hash: &str) -> PathBuf {
    dir.join(format!("webreplay-{session}-{url_hash}"))
}

/// A URL identifies a page; the fragment does not — it addresses a place
/// within one.
fn url_key(url: &str) -> String {
    let no_fragment = url.split('#').next().unwrap_or(url);
    crate::dedup_ref::hash_content(no_fragment.trim().as_bytes())
}

/// What a previous delivery of this URL left behind.
struct Prior {
    at: u64,
    /// Hash of the RAW body. Identity: only these bytes fold.
    content_hash: String,
    /// Hash the stash actually answers to, i.e. of the REDACTED body — empty
    /// when the body could not be stashed. These two are not the same string,
    /// and telling the agent the first one is what made the recovery command
    /// this note advertises a dead pointer (MEASURED: `expand-ref` answered
    /// "hash not found in dedup cache or stash").
    ref_hash: String,
}

fn load_prior(dir: &std::path::Path, session: &str, url: &str) -> Option<Prior> {
    let raw = std::fs::read_to_string(state_path(dir, session, &url_key(url))).ok()?;
    let mut parts = raw.trim().splitn(3, ':');
    let at = parts.next()?.parse().ok()?;
    let content_hash = parts.next()?.to_string();
    Some(Prior {
        at,
        content_hash,
        ref_hash: parts.next().unwrap_or("").to_string(),
    })
}

/// Remember that this exact body was delivered for this URL, so an identical
/// re-fetch in the same session can point at it instead of repeating it.
///
/// The body is also put in the stash, because a pointer the agent cannot
/// redeem is worse than the second copy it replaced.
pub fn remember(session: &str, url: &str, body: &str) {
    remember_in(
        &sessions_dir(),
        session,
        url,
        body,
        crate::platform::now_unix_secs(),
    );
}

fn remember_in(dir: &std::path::Path, session: &str, url: &str, body: &str, now: u64) {
    if std::env::var("TOKENADE_NO_REPLAY").is_ok() || session.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let hash = crate::dedup_ref::hash_content(body.as_bytes());
    // Register the bytes under the hash `expand-ref` looks up, so the note we
    // will print on the next fetch names something that exists.
    let ref_hash = crate::disclose::stash_for_reference(body, &crate::disclose::SourceHint::web())
        .map(|(h, _)| h)
        .unwrap_or_default();
    let _ = std::fs::write(
        state_path(dir, session, &url_key(url)),
        format!("{now}:{hash}:{ref_hash}"),
    );
}

/// PreCompact squashed the transcript: everything delivered before that moment
/// is gone from context, so nothing may be replayed against it.
fn compaction_since(dir: &std::path::Path, session: &str, ts: u64) -> bool {
    std::fs::read_to_string(dir.join(format!("{session}-compact-ts")))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .is_some_and(|c| c >= ts)
}

/// The replacement to deliver instead of `body`, or `None` to deliver `body`.
///
/// `None` is the answer whenever anything is uncertain: a different body, a
/// stale first copy, a compacted transcript, a body too small to be worth a
/// pointer, no session to scope it to.
pub fn fold_if_already_delivered(session: &str, url: &str, body: &str) -> Option<String> {
    fold_in(
        &sessions_dir(),
        session,
        url,
        body,
        crate::platform::now_unix_secs(),
    )
}

fn fold_in(
    dir: &std::path::Path,
    session: &str,
    url: &str,
    body: &str,
    now: u64,
) -> Option<String> {
    if std::env::var("TOKENADE_NO_REPLAY").is_ok() || session.is_empty() {
        return None;
    }
    let tokens = crate::tokenizer::estimate(body);
    if tokens < MIN_TOKENS_TO_FOLD {
        return None;
    }
    let prior = load_prior(dir, session, url)?;
    let age = now.saturating_sub(prior.at);
    if age > max_age_secs() {
        return None;
    }
    if compaction_since(dir, session, prior.at) {
        return None;
    }
    // The whole point: only IDENTICAL content folds. A page that moved between
    // the two fetches is delivered in full.
    let hash = crate::dedup_ref::hash_content(body.as_bytes());
    if hash != prior.content_hash {
        return None;
    }
    // Recovery, only if it is real: an unstashed body has no hash the stash
    // answers to, and naming one anyway strands the agent.
    let recover = if prior.ref_hash.is_empty() {
        "Full text: re-fetch with TOKENADE_NO_REPLAY=1.".to_string()
    } else {
        format!(
            "Full text without re-fetching: `tokenade expand-ref {}`.",
            prior.ref_hash
        )
    };
    Some(format!(
        "[tokenade] {url} was fetched {age}s ago in this session and has not \
         changed since — its content is above in your context. {tokens} tokens \
         not re-sent. {recover}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory per test. No environment is touched, so these run in
    /// parallel with the rest of the suite without lying about each other.
    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-webreplay-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn page(marker: &str) -> String {
        format!(
            "# Title {marker}\n\n{}",
            "body text that is long enough to matter. ".repeat(120)
        )
    }

    const T0: u64 = 1_700_000_000;

    #[test]
    fn the_same_page_twice_is_delivered_once() {
        let _iso = crate::platform::StoreIsolation::new("the_same_page_twice_is_d");
        let d = dir("same");
        let (s, u) = ("sess1", "https://e.co/a");
        let body = page("x");
        assert!(
            fold_in(&d, s, u, &body, T0).is_none(),
            "nothing was delivered yet"
        );
        remember_in(&d, s, u, &body, T0);
        let folded = fold_in(&d, s, u, &body, T0 + 12).expect("second delivery must fold");
        assert!(folded.contains("has not changed"), "{folded}");
        assert!(
            folded.contains("expand-ref"),
            "no way back to the bytes: {folded}"
        );
    }

    /// The pointer has to be redeemable. It was not: the note named the hash
    /// of the raw body, which nothing had ever registered — `expand-ref`
    /// answered "not found in dedup cache or stash" on a real fetch. A fold
    /// that strands the content it replaced is worse than no fold.
    ///
    /// Writes to the real stash on purpose: that is the store `expand-ref`
    /// reads, and asserting against anything else would prove nothing. No
    /// environment variable is touched, so this stays parallel-safe.
    #[test]
    fn the_recovery_pointer_can_actually_be_redeemed() {
        let _iso = crate::platform::StoreIsolation::new("the_recovery_pointer_can");
        let d = dir("recover");
        let body = page("recoverable");
        remember_in(&d, "sess-recover", "https://e.co/a", &body, T0);
        let folded =
            fold_in(&d, "sess-recover", "https://e.co/a", &body, T0 + 5).expect("must fold");
        let hash = folded
            .split("expand-ref ")
            .nth(1)
            .and_then(|s| s.split('`').next())
            .expect("the note must name a hash")
            .trim()
            .to_string();
        let (recovered, _) = crate::disclose::read_stash_by_hash(&hash)
            .unwrap_or_else(|| panic!("advertised `expand-ref {hash}` resolves to nothing"));
        assert!(
            recovered.contains("body text that is long enough"),
            "the stash gave back something else"
        );
    }

    /// The guard that makes this safe: a page that moved is served in full.
    #[test]
    fn a_page_that_changed_is_delivered_again() {
        let _iso = crate::platform::StoreIsolation::new("a_page_that_changed_is_d");
        let d = dir("changed");
        remember_in(&d, "sess1", "https://e.co/a", &page("first"), T0);
        assert!(
            fold_in(&d, "sess1", "https://e.co/a", &page("second"), T0 + 5).is_none(),
            "a changed page must never be replaced by a pointer"
        );
    }

    #[test]
    fn another_url_is_not_the_same_page() {
        let _iso = crate::platform::StoreIsolation::new("another_url_is_not_the_s");
        let d = dir("url");
        let body = page("x");
        remember_in(&d, "sess1", "https://e.co/a", &body, T0);
        assert!(fold_in(&d, "sess1", "https://e.co/b", &body, T0 + 5).is_none());
    }

    /// A fragment addresses a place within one page, not another page.
    #[test]
    fn a_fragment_is_the_same_page() {
        let _iso = crate::platform::StoreIsolation::new("a_fragment_is_the_same_p");
        let d = dir("frag");
        let body = page("x");
        remember_in(&d, "sess1", "https://e.co/a", &body, T0);
        assert!(fold_in(&d, "sess1", "https://e.co/a#part2", &body, T0 + 5).is_some());
    }

    /// Another session cannot be told "see above" about a context it never saw.
    #[test]
    fn another_session_is_never_told_see_above() {
        let _iso = crate::platform::StoreIsolation::new("another_session_is_never");
        let d = dir("sess");
        let body = page("x");
        remember_in(&d, "sess1", "https://e.co/a", &body, T0);
        assert!(fold_in(&d, "sess2", "https://e.co/a", &body, T0 + 5).is_none());
    }

    /// Past the window the first copy is no longer reachable, so the pointer
    /// would point at nothing.
    #[test]
    fn a_stale_first_copy_is_not_pointed_at() {
        let _iso = crate::platform::StoreIsolation::new("a_stale_first_copy_is_no");
        let d = dir("stale");
        let body = page("x");
        remember_in(&d, "sess1", "https://e.co/a", &body, T0);
        let past = T0 + max_age_secs() + 1;
        assert!(fold_in(&d, "sess1", "https://e.co/a", &body, past).is_none());
    }

    /// A summary squashed the transcript: the first copy is provably gone.
    #[test]
    fn nothing_is_replayed_across_a_compaction() {
        let _iso = crate::platform::StoreIsolation::new("nothing_is_replayed_acro");
        let d = dir("compact");
        let body = page("x");
        remember_in(&d, "sess1", "https://e.co/a", &body, T0);
        std::fs::write(d.join("sess1-compact-ts"), (T0 + 1).to_string()).unwrap();
        assert!(fold_in(&d, "sess1", "https://e.co/a", &body, T0 + 5).is_none());
    }

    /// A small page costs more to explain than to repeat.
    #[test]
    fn a_small_page_is_just_sent_again() {
        let _iso = crate::platform::StoreIsolation::new("a_small_page_is_just_sen");
        let d = dir("small");
        let tiny = "# Hi\n\nshort page\n";
        remember_in(&d, "sess1", "https://e.co/a", tiny, T0);
        assert!(fold_in(&d, "sess1", "https://e.co/a", tiny, T0 + 5).is_none());
    }
}
