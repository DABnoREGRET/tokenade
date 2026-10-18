//! One authority per payload, and one credit per payload.
//!
//! The real risk of the ladder is not that a rung is missing, it is that two
//! rungs carry at the same time. A tool result folded by the hook and folded
//! again by this proxy is noise at best, a loss at worst, and in every case a
//! gain counted twice. That has already bitten this project twice with numbers
//! attached (rollup +93 %, `gain` 311 M → 224 M), which is why the rules below
//! are code rather than a paragraph.
//!
//! Three rules:
//!
//! 1. the highest rung that touched a payload owns it, the ones below relay;
//! 2. the marker is carried by the CONTENT, not by the layer —
//!    [`tokenade_core::fold_marker::already_folded`] is that owner, and this
//!    module calls it rather than inventing a flag;
//! 3. credit is unique per (session, content hash). The proxy sees the WHOLE
//!    conversation on every turn, so a naive counter would re-credit the same
//!    tool result forty times. Without rule 3 rank 4 is a machine for inflating
//!    the dashboard.

use std::collections::HashSet;
use std::sync::Mutex;

/// Header an agent-side channel sets to declare itself. HTTP counterpart of
/// `TOKENADE_CHANNELS` (the "session declaration").
pub const CHANNEL_HEADER: &str = "x-tokenade-channel";
/// Header carrying the agent's own session identity, when it has one.
pub const SESSION_HEADER: &str = "x-tokenade-session";
/// Environment variable the local channels use to declare themselves.
pub const CHANNELS_ENV: &str = "TOKENADE_CHANNELS";

/// The name this rung answers to. Anything else in a declaration is somebody
/// above us.
const OURS: &[&str] = &["llm", "llm-proxy", "rank4"];

/// Is a rung ABOVE this one already handling payloads on this session?
///
/// `declared` is the request's channel header; the environment is consulted
/// when it is absent, because a proxy started by the same shell as the agent
/// inherits `TOKENADE_CHANNELS` while an agent that never heard of tokenade
/// sends no header at all.
///
/// Deliberately conservative in BOTH unknown directions: an unrecognised
/// channel name counts as a higher rung. A missed fold costs tokens once; a
/// double fold costs accuracy, because the second pass re-summarises a summary
/// and what it throws away is no longer recoverable from what the first kept.
pub fn a_higher_rung_is_active(declared: Option<&str>) -> bool {
    let raw = match declared {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => match std::env::var(CHANNELS_ENV) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => return false,
        },
    };
    raw.split(',')
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .any(|t| !OURS.contains(&t.as_str()))
}

/// May this proxy rewrite this particular block?
///
/// Rule 2 in one call. The question a new channel must ask is "has anyone
/// touched this", not "have I touched this" — which is why it goes through
/// `fold_marker` and not through a marker of our own.
pub fn may_fold(text: &str) -> bool {
    !tokenade_core::fold_marker::already_folded(text)
}

/// How many payload hashes one process will remember.
///
/// ~100 k entries, a few megabytes. Past it the ledger stops issuing credit
/// rather than forgetting and re-crediting: forgetting would inflate, and
/// inflating is the one direction this project has ruled out.
const MAX_REMEMBERED: usize = 100_000;

/// Rule 3: one credit per (session, content).
#[derive(Default)]
pub struct CreditLedger {
    seen: Mutex<HashSet<u64>>,
}

impl CreditLedger {
    /// True exactly once for a given (session, content) pair.
    ///
    /// Concurrency-safe because an agent may have several requests in flight
    /// and each is served on its own thread; two threads that both fold the
    /// same re-sent result must produce one credit between them.
    #[cfg(test)]
    pub fn first_time(&self, session: &str, content: &str) -> bool {
        self.first_time_key(hash_of(session, content))
    }

    /// [`Self::first_time`] for a block already reduced to its identity
    /// ([`super::wire::block_identity`]).
    pub fn first_time_block(&self, session: &str, block: u64) -> bool {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        session.hash(&mut h);
        block.hash(&mut h);
        self.first_time_key(h.finish())
    }

    /// Was this block already credited in this session? Read-only.
    pub fn seen_block(&self, session: &str, block: u64) -> bool {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        session.hash(&mut h);
        block.hash(&mut h);
        let key = h.finish();
        self.seen.lock().map(|g| g.contains(&key)).unwrap_or(false)
    }

    fn first_time_key(&self, key: u64) -> bool {
        let Ok(mut g) = self.seen.lock() else {
            // A poisoned lock means another thread panicked mid-update. Decline
            // the credit; under-counting is survivable, and this is not a path
            // that may take the session down with it.
            return false;
        };
        if g.len() >= MAX_REMEMBERED {
            return false;
        }
        g.insert(key)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.seen.lock().map(|g| g.len()).unwrap_or(0)
    }
}

#[cfg(test)]
fn hash_of(session: &str, content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    session.hash(&mut h);
    // Length-separated so that ("ab", "c") and ("a", "bc") cannot collide by
    // concatenation — sessions are attacker-adjacent strings from a header.
    content.len().hash(&mut h);
    content.hash(&mut h);
    h.finish()
}

/// The session key for a request.
///
/// Falls back to a single shared bucket when the agent declares nothing. That
/// under-credits — two different sessions sending byte-identical tool output
/// are counted once — and that is the direction chosen on purpose.
pub fn session_key(declared: Option<&str>) -> String {
    declared
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("anonymous")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_name_is_not_a_higher_rung() {
        assert!(!a_higher_rung_is_active(Some("llm")));
        assert!(!a_higher_rung_is_active(Some("llm-proxy")));
        assert!(!a_higher_rung_is_active(Some(" LLM-Proxy ")));
    }

    #[test]
    fn any_other_declared_channel_stops_us_folding() {
        for other in [
            "hook",
            "plugin",
            "shell",
            "mcp",
            "hook,llm",
            "something-new",
        ] {
            assert!(
                a_higher_rung_is_active(Some(other)),
                "{other} must claim the payload"
            );
        }
    }

    #[test]
    fn an_absent_declaration_leaves_the_proxy_free() {
        // The common case, and the reason rank 4 exists: an agent that has
        // never heard of tokenade sends no header at all.
        assert!(!a_higher_rung_is_active(None));
        assert!(!a_higher_rung_is_active(Some("")));
        assert!(!a_higher_rung_is_active(Some("   ")));
    }

    #[test]
    fn content_that_someone_already_folded_is_left_alone() {
        // Rule 2 through `fold_marker`, not through a private flag. The
        // measured failure behind it: folding a fold gives 16 198 → 4 124 →
        // 4 070 → 4 071 — a sequence that does not even converge.
        assert!(may_fold("plain tool output, nobody has touched this"));
        let folded = "… 412 lines elided · tokenade expand-ref a1b2c3d4e5f60718";
        assert!(
            !may_fold(folded),
            "a payload carrying a recovery pointer belongs to whoever put it there"
        );
    }

    #[test]
    fn the_same_result_re_sent_every_turn_is_credited_once() {
        // The shape of a real conversation: the proxy sees turn 1's tool result
        // again on turns 2..40. Forty credits for one saving is exactly the
        // double-counting rule 3 exists to prevent.
        let l = CreditLedger::default();
        let result = "a tool result that will be re-sent on every later turn";
        assert!(l.first_time("s1", result));
        for _ in 0..39 {
            assert!(!l.first_time("s1", result));
        }
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn two_sessions_do_not_shadow_each_other() {
        let l = CreditLedger::default();
        assert!(l.first_time("s1", "same bytes"));
        assert!(
            l.first_time("s2", "same bytes"),
            "a second session is not a repeat"
        );
    }

    #[test]
    fn a_full_ledger_stops_crediting_rather_than_forgetting() {
        // Anti-inflation. An LRU that evicted would re-credit the evicted
        // payload the next turn it is re-sent, and re-sending is what this
        // whole rung is about.
        let l = CreditLedger::default();
        for i in 0..MAX_REMEMBERED {
            assert!(l.first_time("s", &format!("payload {i}")));
        }
        assert!(!l.first_time("s", "a brand new payload"));
        assert_eq!(l.len(), MAX_REMEMBERED);
    }

    #[test]
    fn a_session_key_is_never_empty() {
        assert_eq!(session_key(Some("abc")), "abc");
        assert_eq!(session_key(None), "anonymous");
        assert_eq!(session_key(Some("  ")), "anonymous");
    }
}

#[cfg(test)]
mod no_double_declaration {
    //! One payload, one authority, one credit — asserted from the proxy's side
    //! now that it folds on agents that also have a hook.
    //!
    //! Before 2026-08-20 this could not happen by construction: the proxy was
    //! switched off wherever a hook existed. That coarse rule is gone, so the
    //! invariant it used to provide has to be asserted directly.

    use super::*;

    const BANNER: &str = "[tokenade: 34789 tokens folded away — full output: \
                          `tokenade expand-ref 8cbb0d6ffff8` (`--grep <pattern>` to filter)]";

    #[test]
    fn a_payload_the_hook_folded_is_not_folded_again() {
        // The POSITIVE gain counted twice: the hook shrinks it, reports its
        // saving, and the proxy shrinks the banner and reports the same bytes
        // a second time. `may_fold` is what stops it, and it asks about the
        // CONTENT rather than about which agent is in front.
        assert!(!may_fold(&format!("build output…\n{BANNER}")));
        assert!(!may_fold("identical to a prior call — §ref:8cbb0d6ffff8§"));
        // And a plain result is still ours.
        assert!(may_fold("error[E0432]: unresolved import"));
    }

    #[test]
    fn the_same_payload_is_credited_once_per_session() {
        // The other half of the same coin. Even when folding is allowed, a
        // conversation re-sent turn after turn carries the same tool results,
        // and crediting each turn would inflate the ledger by the length of
        // the conversation.
        let ledger = CreditLedger::default();
        let payload = "a large tool result";
        assert!(ledger.first_time("s1", payload));
        assert!(!ledger.first_time("s1", payload), "credited twice");
        // A different session is a different conversation and genuinely paid
        // for those bytes again.
        assert!(ledger.first_time("s2", payload));
    }

    #[test]
    fn a_running_channel_still_takes_precedence_over_us() {
        // The runtime signal, which is stronger evidence than any table: when
        // an agent-side channel announces itself on the request, the proxy
        // stands down for the whole session rather than negotiating per block.
        assert!(a_higher_rung_is_active(Some("hook")));
        assert!(!a_higher_rung_is_active(None));
    }
}
